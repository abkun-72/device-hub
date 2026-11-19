//! HID++ フレームと format 3 sector の純粋な変換。I/O はここに置かない。
use devhub_core::{backend::*, model::*};

pub const SECTOR_SIZE: usize = 255;

pub fn frame(feature: u8, function: u8, software: u8, params: &[u8]) -> HubResult<[u8; 20]> {
    if function > 15 || !(1..=15).contains(&software) || params.len() > 16 {
        return Err(HubError::Invalid("HID++ frame".into()));
    }
    let mut out = [0; 20];
    out[..4].copy_from_slice(&[0x11, 1, feature, function << 4 | software]);
    out[4..4 + params.len()].copy_from_slice(params);
    Ok(out)
}

/// None は別リクエストへの応答／自発通知。short のエラーも受理する。
pub fn response<'a>(data: &'a [u8], request: &[u8; 20]) -> HubResult<Option<&'a [u8]>> {
    let expected = match data.first() { Some(0x10) => 7, Some(0x11) => 20, _ => return Ok(None) };
    if data.len() != expected || data[1] != request[1] { return Ok(None); }
    if data[2] == 0xff && data[3] == request[2] && data[4] == request[3] {
        return Err(HubError::Protocol(format!("HID++ feature {:02x} fn {:02x}: error {:02x}", data[3], data[4], data[5])));
    }
    Ok((data[2] == request[2] && data[3] == request[3]).then_some(&data[4..]))
}

pub fn crc(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 { crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 }; }
    }
    crc
}

pub fn validate_sector(data: &[u8]) -> HubResult<()> {
    if data.len() != SECTOR_SIZE || crc(&data[..SECTOR_SIZE - 2]) != u16::from_be_bytes([data[253], data[254]]) {
        return Err(HubError::Protocol("G304 sector CRC 不一致（書き込みを拒否）".into()));
    }
    Ok(())
}

pub fn user_sector(directory: &[u8]) -> HubResult<u16> {
    validate_sector(directory)?;
    for entry in directory[..253].chunks_exact(4) {
        let sector = u16::from_be_bytes([entry[0], entry[1]]);
        if sector == 0xffff { break; }
        if entry[2] == 1 {
            writable(sector)?;
            return Ok(sector);
        }
    }
    Err(HubError::Protocol("G304: 有効なユーザープロファイルがありません".into()))
}

pub fn writable(sector: u16) -> HubResult<()> {
    if !(1..0x100).contains(&sector) { return Err(HubError::Invalid("ROM / directory への書き込みは禁止です".into())); }
    Ok(())
}

pub fn dpi_values(data: &[u8]) -> HubResult<Vec<u32>> {
    validate_sector(data)?;
    let values: Vec<_> = data[3..13].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32).collect();
    let count = values.iter().position(|v| *v == 0).unwrap_or(5);
    if count == 0 || values[count..].iter().any(|v| *v != 0)
        || values[..count].iter().any(|v| !valid_dpi(*v)) || data[1] as usize >= count {
        return Err(HubError::Protocol("G304: 不正な DPI ステージ".into()));
    }
    Ok(values[..count].to_vec())
}

pub fn live_dpi_valid(dpi: u32) -> bool { valid_dpi(dpi) }

fn valid_dpi(dpi: u32) -> bool { (100..=12000).contains(&dpi) && dpi.is_multiple_of(50) }

const MEDIA: &[(MediaFn, u16)] = &[
    (MediaFn::PlayPause, 0xcd), (MediaFn::Stop, 0xb7), (MediaFn::Next, 0xb5), (MediaFn::Prev, 0xb6),
    (MediaFn::VolumeUp, 0xe9), (MediaFn::VolumeDown, 0xea), (MediaFn::Mute, 0xe2),
    (MediaFn::Calculator, 0x192), (MediaFn::MyComputer, 0x194), (MediaFn::Browser, 0x196),
    (MediaFn::Mail, 0x18a), (MediaFn::BrightnessUp, 0x6f), (MediaFn::BrightnessDown, 0x70), (MediaFn::Refresh, 0x227),
];

pub fn decode_button(b: &[u8], shift_dpi: u32) -> ButtonAction {
    match b {
        [0xff, ..] | [0x80, 0, _, _] | [0x90, 0, _, _] => ButtonAction::Disabled,
        [0x80, 1, 0, mask] => {
            let f = match mask { 1 => MouseFn::Left, 2 => MouseFn::Right, 4 => MouseFn::Middle, 8 => MouseFn::Back, 16 => MouseFn::Forward, _ => return unknown(b) };
            ButtonAction::Mouse(f)
        }
        [0x80, 2, mods, key] => ButtonAction::Key(KeyStroke { mods: *mods, usage: *key as u16 }),
        [0x80, 3, hi, lo] => MEDIA.iter().find(|(_, u)| *u == u16::from_be_bytes([*hi, *lo])).map(|(f, _)| ButtonAction::Media(*f)).unwrap_or_else(|| unknown(b)),
        [0x90, 3, 0, 0] => ButtonAction::Dpi(DpiFn::Up),
        [0x90, 4, 0, 0] => ButtonAction::Dpi(DpiFn::Down),
        [0x90, 5, 0, 0] => ButtonAction::Dpi(DpiFn::Cycle),
        [0x90, 7, 0, 0] if shift_dpi != 0 => ButtonAction::Dpi(DpiFn::Shift(shift_dpi)),
        _ => unknown(b),
    }
}

fn unknown(b: &[u8]) -> ButtonAction { ButtonAction::Unsupported { ty: b[0], params: b[1..].to_vec() } }

fn encode_button(action: &ButtonAction) -> HubResult<[u8; 4]> {
    Ok(match action {
        ButtonAction::Disabled => [0x80, 0, 0, 0],
        ButtonAction::Mouse(f) => [0x80, 1, 0, match f {
            MouseFn::Left => 1, MouseFn::Right => 2, MouseFn::Middle => 4, MouseFn::Back => 8, MouseFn::Forward => 16,
            _ => return Err(HubError::Unsupported("G304 mouse function")),
        }],
        ButtonAction::Key(k) if k.usage <= 255 => [0x80, 2, k.mods, k.usage as u8],
        ButtonAction::Media(f) => {
            let usage = MEDIA.iter().find(|(m, _)| m == f).ok_or(HubError::Unsupported("consumer usage"))?.1.to_be_bytes();
            [0x80, 3, usage[0], usage[1]]
        }
        ButtonAction::Dpi(f) => [0x90, match f { DpiFn::Up => 3, DpiFn::Down => 4, DpiFn::Cycle => 5, DpiFn::Shift(_) => 7 }, 0, 0],
        _ => return Err(HubError::Unsupported("G304 button action")),
    })
}

pub fn state(data: &[u8], current_dpi: Option<u32>) -> HubResult<DeviceState> {
    let dpis = dpi_values(data)?;
    let polling = match data[0] { 1 | 2 | 4 | 8 => 1000 / data[0] as i64, _ => return Err(HubError::Protocol("G304 report rate".into())) };
    let shift = dpis.get(data[2] as usize).copied().unwrap_or(0);
    Ok(DeviceState {
        profile: ProfileState::default(),
        settings: [("polling_wireless".into(), SettingValue::Choice(polling)),
            ("default_dpi_stage".into(), SettingValue::Choice(data[1] as i64 + 1))].into(),
        mouse: Some(MouseState {
            active_stage: current_dpi.and_then(|v| dpis.iter().position(|d| *d == v)).unwrap_or(data[1] as usize),
            stages: dpis.iter().map(|d| DpiStage { x: *d, y: None, color: Rgb::hex(0), enabled: true }).collect(),
            buttons: data[32..56].chunks_exact(4).map(|b| decode_button(b, shift)).collect(),
            macros: vec![], adaptive: None,
        }),
        keyboard: None, lighting: Default::default(), peq: None,
    })
}

/// CRC 検証済みの原本から、指定項目と CRC だけを変更する。
pub fn changed(data: &[u8], change: &Change) -> HubResult<Vec<u8>> {
    let dpis = dpi_values(data)?;
    let mut out = data.to_vec();
    match change {
        Change::DpiStages { stages, active } => {
            if !(1..=5).contains(&stages.len()) || *active >= stages.len()
                || stages.iter().any(|s| !valid_dpi(s.x) || !s.enabled || s.y.is_some_and(|y| y != s.x)) {
                return Err(HubError::Invalid("DPI は100〜12000、50刻み、1〜5ステージ".into()));
            }
            out[1] = *active as u8;
            // 削除された shift の参照を残さない。既存の参照はそのまま保持する。
            if out[2] as usize >= stages.len() { out[2] = *active as u8; }
            out[3..13].fill(0);
            for (i, s) in stages.iter().enumerate() { out[3 + i*2..5 + i*2].copy_from_slice(&(s.x as u16).to_le_bytes()); }
        }
        Change::ActiveStage(i) if *i < dpis.len() => out[1] = *i as u8,
        Change::Setting { id, value: SettingValue::Choice(v) } if id == "default_dpi_stage" => {
            if !(1..=dpis.len() as i64).contains(v) { return Err(HubError::Invalid("存在する DPI ステージを指定してください".into())); }
            out[1] = (*v - 1) as u8;
        }
        Change::Setting { id, value: SettingValue::Choice(v) } if id == "polling_wireless" => {
            out[0] = match v { 125 => 8, 250 => 4, 500 => 2, 1000 => 1, _ => return Err(HubError::Invalid("G304 polling rate".into())) };
        }
        Change::Button { slot, action } if *slot < 6 => {
            if let ButtonAction::Dpi(DpiFn::Shift(dpi)) = action {
                let index = dpis.iter().position(|v| v == dpi).ok_or_else(|| HubError::Invalid("DPI ロックには既存ステージの DPI を指定してください".into()))?;
                // shift index は全ボタン共通。他の shift ボタンもこの DPI を使う。
                out[2] = index as u8;
            }
            let bytes = encode_button(action)?;
            out[32 + slot*4..36 + slot*4].copy_from_slice(&bytes);
            if !out[32..56].chunks_exact(4).any(|b| b == [0x80, 1, 0, 1]) {
                return Err(HubError::Invalid("左クリックを最低1つ残してください".into()));
            }
        }
        Change::ActiveStage(_) | Change::Button { .. } => return Err(HubError::Invalid("G304 slot index".into())),
        _ => return Err(HubError::Unsupported("G304 change")),
    }
    let sum = crc(&out[..253]).to_be_bytes();
    out[253..].copy_from_slice(&sum);
    Ok(out)
}
