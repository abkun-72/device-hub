//! G304 の宣言。実機ドライバとデモで共有する。
use crate::model::*;

pub fn g304_caps(battery: bool) -> Capabilities {
    Capabilities {
        settings: vec![
            SettingSpec::choice("polling_wireless", section::PERFORMANCE, "ポーリングレート",
                [125, 250, 500, 1000].map(|hz| ChoiceOpt::new(hz, format!("{hz} Hz"))).into()),
            SettingSpec::choice("default_dpi_stage", section::SENSOR, "起動時 DPI ステージ",
                (1..=5).map(|i| ChoiceOpt::new(i, format!("ステージ {i}"))).collect())
                .help("存在する DPI ステージを指定してください。本体のオンボードメモリに保存します"),
        ],
        profiles: Some(ProfileCaps { count: 1, renamable: false }),
        mouse: Some(MouseCaps {
            dpi_min: 100, dpi_max: 12000, dpi_step: 50,
            min_stages: 1, max_stages: 5, xy_split: false,
            stage_colors: false, stage_toggle: false,
            buttons: ["左クリック", "右クリック", "ホイールクリック", "戻る", "進む", "DPI ボタン"]
                .map(|name| ButtonSlot { name: name.into() }).into(),
            button_kinds: vec![ButtonActionKind::Disabled, ButtonActionKind::Mouse,
                ButtonActionKind::Dpi, ButtonActionKind::Key, ButtonActionKind::Media],
            require_left_click: true, macros: None, live_buttons: vec![], adaptive_dpi: None,
        }),
        battery,
        ..Capabilities::empty()
    }
}
