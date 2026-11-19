//! G304 LIGHTSPEED: HID++ 2.0 / 0x8100 format 3 オンボード設定。
//! 参照実装・出典・未検証範囲は docs/PROTOCOL_NOTES.md §6。
mod codec;

use codec::*;
use devhub_core::{backend::*, model::*};
use std::sync::Mutex;
use std::time::{Duration, Instant};

trait Link {
    fn request(&mut self, feature: u8, function: u8, params: &[u8]) -> HubResult<Vec<u8>>;
}

struct HidLink {
    dev: hidapi::HidDevice,
    software: u8,
    timeout: Duration,
}

impl Link for HidLink {
    fn request(&mut self, feature: u8, function: u8, params: &[u8]) -> HubResult<Vec<u8>> {
        self.software = self.software % 15 + 1;
        let msg = frame(feature, function, self.software, params)?;
        let mut buf = [0; 64];
        // 古い応答を除去し、software ID も変えて同じ fn の連続 read を区別。
        for _ in 0..32 {
            if self.dev.read_timeout(&mut buf, 0).map_err(io)? == 0 { break; }
        }
        if self.dev.write(&msg).map_err(io)? != msg.len() {
            return Err(HubError::Io("G304: short HID write".into()));
        }
        let deadline = Instant::now() + self.timeout;
        while Instant::now() < deadline {
            let wait_ms = deadline.saturating_duration_since(Instant::now()).as_millis().clamp(1, 100) as i32;
            let n = self.dev.read_timeout(&mut buf, wait_ms).map_err(io)?;
            if let Some(params) = response(&buf[..n], &msg)? { return Ok(params.to_vec()); }
        }
        // 書き込みは再送しない。特に fn7 の二重送信は次チャンクを破壊する。
        Err(HubError::Disconnected)
    }
}

fn io(e: hidapi::HidError) -> HubError { HubError::Io(format!("G304 HID++: {e}")) }

/// 列挙時の短い生存確認。永続設定に触れず、device index 1 の IRoot のみ。
/// ドライバが開いているコレクションには使わない（応答の取り合いを避ける）。
pub(crate) fn connected(dev: hidapi::HidDevice) -> bool {
    if dev.set_blocking_mode(false).is_err() { return false; }
    let mut link = HidLink { dev, software: 9, timeout: Duration::from_millis(250) };
    feature(&mut link, 0x0005).is_ok_and(|index| index != 0)
}

fn feature(link: &mut dyn Link, id: u16) -> HubResult<u8> {
    let r = link.request(0, 0, &id.to_be_bytes())?;
    r.first().copied().ok_or_else(|| HubError::Protocol("IRoot short response".into()))
}

fn read_sector(link: &mut dyn Link, onboard: u8, sector: u16) -> HubResult<Vec<u8>> {
    let mut data = vec![0; SECTOR_SIZE];
    for offset in (0..SECTOR_SIZE).step_by(16) {
        let offset = offset.min(SECTOR_SIZE - 16); // 最終 239..255 は直前の1Bと重なる。
        let mut args = Vec::from(sector.to_be_bytes());
        args.extend_from_slice(&(offset as u16).to_be_bytes());
        let r = link.request(onboard, 5, &args)?;
        if r.len() < 16 { return Err(HubError::Protocol("G304 short memory read".into())); }
        data[offset..offset + 16].copy_from_slice(&r[..16]);
    }
    Ok(data)
}

fn read_profile(link: &mut dyn Link, onboard: u8) -> HubResult<(u16, Vec<u8>)> {
    let directory = read_sector(link, onboard, 0)?;
    let sector = user_sector(&directory)?;
    let data = read_sector(link, onboard, sector)?;
    validate_sector(&data)?;
    Ok((sector, data))
}

fn apply_profile(link: &mut dyn Link, onboard: u8, change: &Change) -> HubResult<()> {
    // 書く直前に directory と現物を全読みする。キャッシュを原本にしない。
    let (sector, original) = read_profile(link, onboard)?;
    writable(sector)?;
    let data = changed(&original, change)?;
    if original != data {
        let mut args = Vec::from(sector.to_be_bytes());
        args.extend_from_slice(&[0, 0]);
        args.extend_from_slice(&(SECTOR_SIZE as u16).to_be_bytes());
        link.request(onboard, 6, &args)?;
        for chunk in data.chunks(16) {
            let mut block = [0; 16];
            block[..chunk.len()].copy_from_slice(chunk);
            link.request(onboard, 7, &block)?;
        }
        link.request(onboard, 8, &[])?;
        if read_sector(link, onboard, sector)? != data {
            return Err(HubError::Protocol("G304 書き込み後の読み戻しが一致しません".into()));
        }
    }
    // mode 切替も apply のみ。open/read/example は永続・動作設定を変えない。
    link.request(onboard, 1, &[1])?;
    link.request(onboard, 3, &sector.to_be_bytes())?;
    Ok(())
}

pub struct LogitechDriver {
    link: Mutex<HidLink>,
    info: Mutex<DeviceInfo>,
    onboard: u8,
    dpi: u8,
    battery_feature: Option<(u16, u8)>,
}

/// 渡された long collection だけを検証する。別の受信機へ勝手に切り替えない。
pub fn open(dev: hidapi::HidDevice, mut info: DeviceInfo) -> HubResult<LogitechDriver> {
    dev.set_blocking_mode(false).map_err(io)?;
    let mut link = HidLink { dev, software: 9, timeout: Duration::from_secs(2) };
    // device index 1 が実際に応答する受信機だけを採用する。
    let name_feature = feature(&mut link, 0x0005)?;
    if name_feature == 0 { return Err(HubError::Unsupported("G304 device name")); }
    let len = link.request(name_feature, 0, &[])?[0] as usize;
    let mut name = Vec::new();
    while name.len() < len {
        let r = link.request(name_feature, 1, &[name.len() as u8])?;
        if r.is_empty() { return Err(HubError::Protocol("G304 short device name".into())); }
        name.extend_from_slice(&r[..r.len().min(len - name.len())]);
    }
    let name = String::from_utf8_lossy(&name);
    if !name.contains("G304") { return Err(HubError::Unsupported("receiver is not paired with G304")); }
    info.name = name.into_owned();
    let onboard = feature(&mut link, 0x8100)?;
    if onboard == 0 { return Err(HubError::Unsupported("G304 onboard profiles")); }
    let desc = link.request(onboard, 0, &[])?;
    if desc.len() < 9 || desc[..6] != [1, 3, 1, 1, 1, 6]
        || u16::from_be_bytes([desc[7], desc[8]]) as usize != SECTOR_SIZE {
        return Err(HubError::Unsupported("G304 memory model / profile format / sector size"));
    }
    let dpi = feature(&mut link, 0x2201)?;
    let mut battery_feature = None;
    for id in [0x1000, 0x1004, 0x1001] {
        let index = feature(&mut link, id)?;
        if index != 0 { battery_feature = Some((id, index)); break; }
    }
    // CRC を検査する。設定や mode を変更する要求は一切送らない。
    read_profile(&mut link, onboard)?;
    Ok(LogitechDriver { link: Mutex::new(link), info: Mutex::new(info), onboard, dpi, battery_feature })
}

fn battery_value(id: u16, data: &[u8]) -> HubResult<Battery> {
    if data.len() < 3 { return Err(HubError::Protocol("G304 short battery response".into())); }
    Ok(match id {
        0x1000 | 0x1004 => Battery {
            percent: (data[0] <= 100).then_some(data[0]),
            charging: matches!(data[2], 1 | 2 | 4),
        },
        // 0x1001 は電圧 (BE16)。AA の残量%へ Li-ion 用換算を適用しない。
        0x1001 => Battery { percent: None, charging: data[2] & 0x80 != 0 && data[2] & 3 == 0 },
        _ => return Err(HubError::Unsupported("G304 battery feature")),
    })
}

impl DeviceDriver for LogitechDriver {
    fn info(&self) -> DeviceInfo { self.info.lock().unwrap_or_else(|p| p.into_inner()).clone() }
    fn caps(&self) -> Capabilities { devhub_core::logitech::g304_caps(self.battery_feature.is_some()) }
    fn read(&self) -> HubResult<DeviceState> {
        let mut link = self.link.lock().unwrap_or_else(|p| p.into_inner());
        let (_, data) = read_profile(&mut *link, self.onboard)?;
        let current = if self.dpi != 0 {
            let r = link.request(self.dpi, 2, &[0])?;
            if r.len() < 3 || r[0] != 0 { return Err(HubError::Protocol("G304 sensor DPI response".into())); }
            Some(u16::from_be_bytes([r[1], r[2]]) as u32)
        } else { None };
        state(&data, current)
    }
    fn apply(&self, change: &Change) -> HubResult<()> {
        apply_profile(&mut *self.link.lock().unwrap_or_else(|p| p.into_inner()), self.onboard, change)
    }
    fn battery(&self) -> HubResult<Battery> {
        let (id, feature) = self.battery_feature.ok_or(HubError::Unsupported("G304 battery"))?;
        let data = self.link.lock().unwrap_or_else(|p| p.into_inner()).request(feature, u8::from(id == 0x1004), &[])?;
        let battery = battery_value(id, &data)?;
        self.info.lock().unwrap_or_else(|p| p.into_inner()).battery = Some(battery);
        Ok(battery)
    }
}

#[cfg(test)]
mod tests;
