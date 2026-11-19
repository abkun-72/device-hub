use super::*;

fn fixture() -> Vec<u8> {
    // .private-analysis/logitech/onboard_backup.json の sectors["0x1"]。
    // private ファイルへのビルド時依存を持たない、実測255B。
    let hex = include_str!("../../fixtures/logitech/sector1.hex").trim();
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i+2], 16).unwrap()).collect()
}

fn directory(sector: u16) -> Vec<u8> {
    let mut data = vec![0xff; 255];
    data[..4].copy_from_slice(&[sector.to_be_bytes()[0], sector.to_be_bytes()[1], 1, 0]);
    let sum = crc(&data[..253]).to_be_bytes();
    data[253..].copy_from_slice(&sum);
    data
}

#[test]
fn backup_crc_and_decode() {
    let data = fixture();
    assert_eq!(data.len(), 255);
    assert_eq!(crc(&data[..253]), 0x421f);
    assert_eq!(&data[253..], &[0x42, 0x1f]);
    let s = state(&data, None).unwrap();
    let m = s.mouse.unwrap();
    assert_eq!(m.stages.iter().map(|s| s.x).collect::<Vec<_>>(), [1400, 1600, 3200]);
    assert_eq!(m.active_stage, 1);
    assert_eq!(s.settings["default_dpi_stage"], SettingValue::Choice(2));
    assert_eq!(m.buttons, [ButtonAction::Mouse(MouseFn::Left), ButtonAction::Mouse(MouseFn::Right),
        ButtonAction::Mouse(MouseFn::Middle), ButtonAction::Mouse(MouseFn::Back),
        ButtonAction::Mouse(MouseFn::Forward), ButtonAction::Dpi(DpiFn::Cycle)]);
    assert_eq!(state(&data, Some(3200)).unwrap().mouse.unwrap().active_stage, 2);
}

#[test]
fn changes_preserve_unrelated_bytes_and_recalculate_crc() {
    let data = fixture();
    let mut stages = state(&data, None).unwrap().mouse.unwrap().stages;
    stages[0].x = 400;
    let out = changed(&data, &Change::DpiStages { stages, active: 2 }).unwrap();
    validate_sector(&out).unwrap();
    assert_eq!(dpi_values(&out).unwrap(), [400, 1600, 3200]);
    assert_eq!(out[1], 2);
    for i in (0..253).filter(|i| *i != 1 && !(3..13).contains(i)) { assert_eq!(out[i], data[i], "byte {i}"); }
    let out = changed(&data, &Change::Setting { id: "polling_wireless".into(), value: SettingValue::Choice(500) }).unwrap();
    assert_eq!(out[0], 2);
    assert_eq!(&out[1..253], &data[1..253]);
    validate_sector(&out).unwrap();
    let out = changed(&data, &Change::Setting { id: "default_dpi_stage".into(), value: SettingValue::Choice(3) }).unwrap();
    assert_eq!(out[1], 2);
    validate_sector(&out).unwrap();
}

#[test]
fn button_roundtrips_and_unknown_preservation() {
    for action in [ButtonAction::Disabled, ButtonAction::Key(KeyStroke { mods: 0x82, usage: 4 }),
        ButtonAction::Media(MediaFn::Calculator), ButtonAction::Dpi(DpiFn::Cycle),
        ButtonAction::Dpi(DpiFn::Up), ButtonAction::Dpi(DpiFn::Down), ButtonAction::Dpi(DpiFn::Shift(1600))] {
        let data = fixture();
        let out = changed(&data, &Change::Button { slot: 5, action: action.clone() }).unwrap();
        validate_sector(&out).unwrap();
        assert_eq!(state(&out, None).unwrap().mouse.unwrap().buttons[5], action);
        assert_eq!(&out[56..253], &data[56..253]);
    }
    let mut data = fixture();
    data[52..56].copy_from_slice(&[0, 1, 2, 3]); // 未対応マクロも別設定変更時に保持。
    let sum = crc(&data[..253]).to_be_bytes();
    data[253..].copy_from_slice(&sum);
    let out = changed(&data, &Change::ActiveStage(0)).unwrap();
    assert_eq!(&out[32..253], &data[32..253]);
    assert!(matches!(state(&out, None).unwrap().mouse.unwrap().buttons[5], ButtonAction::Unsupported { .. }));
}

#[test]
fn invalid_values_crc_and_protected_sectors_are_rejected() {
    let data = fixture();
    for sector in [0, 0x100, 0x101, 0xffff] { assert!(writable(sector).is_err()); assert!(user_sector(&directory(sector)).is_err()); }
    assert_eq!(user_sector(&directory(1)).unwrap(), 1);
    for index in [3, 5, usize::MAX] { assert!(changed(&data, &Change::ActiveStage(index)).is_err()); }
    assert!(changed(&data, &Change::Button { slot: 0, action: ButtonAction::Disabled }).is_err());
    for dpi in [0, 99, 125, 12050, u32::MAX] {
        let mut stages = state(&data, None).unwrap().mouse.unwrap().stages;
        stages[0].x = dpi;
        assert!(changed(&data, &Change::DpiStages { stages, active: 0 }).is_err());
    }
    let mut corrupt = data;
    corrupt[20] ^= 1;
    assert!(changed(&corrupt, &Change::ActiveStage(0)).is_err());
    assert!(validate_sector(&[0; 254]).is_err());
}

#[test]
fn frames_match_all_response_identifiers() {
    let request = frame(7, 5, 10, &[0, 1, 0, 239]).unwrap();
    assert_eq!(&request[..8], &[0x11, 1, 7, 0x5a, 0, 1, 0, 239]);
    assert!(request[8..].iter().all(|b| *b == 0));
    assert_eq!(response(&request, &request).unwrap().unwrap(), &request[4..]);
    for index in [0, 1, 2, 3] {
        let mut other = request; other[index] ^= 1;
        assert!(response(&other, &request).unwrap().is_none());
    }
    assert!(response(&request[..19], &request).unwrap().is_none());
    let error = [0x10, 1, 0xff, 7, 0x5a, 9, 0];
    assert!(response(&error, &request).is_err());
    let mut other = error; other[4] ^= 1;
    assert!(response(&other, &request).unwrap().is_none());
    assert!(frame(7, 16, 10, &[]).is_err());
    assert!(frame(7, 0, 0, &[]).is_err());
    assert!(frame(7, 0, 10, &[0; 17]).is_err());
}

struct Memory {
    directory: Vec<u8>, profile: Vec<u8>, written: Vec<u8>, calls: Vec<(u8, Vec<u8>)>, mismatch: bool,
}
impl Memory {
    fn new() -> Self { Self { directory: directory(1), profile: fixture(), written: vec![], calls: vec![], mismatch: false } }
}
impl Link for Memory {
    fn request(&mut self, feature: u8, function: u8, params: &[u8]) -> HubResult<Vec<u8>> {
        assert_eq!(feature, 7);
        self.calls.push((function, params.to_vec()));
        match function {
            5 => {
                let sector = u16::from_be_bytes([params[0], params[1]]);
                let offset = u16::from_be_bytes([params[2], params[3]]) as usize;
                assert!(offset <= 239);
                let source = if sector == 0 { &self.directory } else { &self.profile };
                Ok(source[offset..offset+16].to_vec())
            }
            6 => { assert_eq!(params, &[0, 1, 0, 0, 0, 255]); self.written.clear(); Ok(vec![0;16]) }
            7 => { assert_eq!(params.len(), 16); self.written.extend_from_slice(params); Ok(vec![0;16]) }
            8 => { self.profile = self.written[..255].to_vec(); if self.mismatch { self.profile[3] ^= 1; } Ok(vec![0;16]) }
            1 | 3 => Ok(vec![0;16]),
            _ => panic!("unexpected function {function}"),
        }
    }
}

#[test]
fn memory_transaction_padding_readback_and_activation() {
    let mut memory = Memory::new();
    apply_profile(&mut memory, 7, &Change::ActiveStage(2)).unwrap();
    assert_eq!(memory.written.len(), 256);
    assert_eq!(memory.written[255], 0);
    validate_sector(&memory.profile).unwrap();
    assert_eq!(memory.profile[1], 2);
    assert_eq!(memory.calls.iter().filter(|(f,_)| *f == 7).count(), 16);
    assert_eq!(&memory.calls[memory.calls.len()-2..], &[(1, vec![1]), (3, vec![0,1])]);
    assert!(memory.calls.iter().any(|(f,p)| *f == 5 && p == &[0,1,0,239]));
}

#[test]
fn unsafe_memory_never_starts_a_write_and_mismatch_never_activates() {
    for sector in [0, 0x100, 0x101] {
        let mut memory = Memory::new(); memory.directory = directory(sector);
        assert!(apply_profile(&mut memory, 7, &Change::ActiveStage(2)).is_err());
        assert!(memory.calls.iter().all(|(f,_)| *f == 5));
    }
    let mut memory = Memory::new(); memory.profile[40] ^= 1;
    assert!(apply_profile(&mut memory, 7, &Change::ActiveStage(2)).is_err());
    assert!(memory.calls.iter().all(|(f,_)| *f == 5));
    let mut memory = Memory::new(); memory.mismatch = true;
    assert!(apply_profile(&mut memory, 7, &Change::ActiveStage(2)).is_err());
    assert!(!memory.calls.iter().any(|(f,_)| *f == 1 || *f == 3));
    let mut memory = Memory::new();
    read_profile(&mut memory, 7).unwrap();
    assert!(memory.calls.iter().all(|(f,_)| *f == 5));
}

#[test]
fn battery_percent_is_not_voltage() {
    assert_eq!(battery_value(0x1000, &[80, 20, 0]).unwrap(), Battery { percent: Some(80), charging: false });
    assert_eq!(battery_value(0x1004, &[50, 4, 1]).unwrap(), Battery { percent: Some(50), charging: true });
    assert_eq!(battery_value(0x1001, &[0x05, 0xdc, 0]).unwrap().percent, None);
}

#[test]
fn registry_uses_only_the_long_collection_and_caps_match() {
    use devhub_core::registry::match_device;
    assert!(match_device(0x046d, 0xc53f, 0xff00, 1).is_none());
    assert_eq!(match_device(0x046d, 0xc53f, 0xff00, 2).unwrap().driver, DriverKind::Logitech);
    assert!(match_device(0x046d, 0xc53f, 1, 2).is_none());
    let caps = devhub_core::logitech::g304_caps(false);
    let mouse = caps.mouse.unwrap();
    assert_eq!((mouse.dpi_min, mouse.dpi_max, mouse.dpi_step), (100, 12000, 50));
    assert_eq!(mouse.buttons.len(), 6);
    assert!(!caps.battery);
    assert!(caps.lighting.is_empty());
    assert!(mouse.macros.is_none());
}
