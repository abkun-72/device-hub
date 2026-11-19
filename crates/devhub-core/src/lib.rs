//! devhub-core: Device HUB のコア (GUI 依存なし)。
//!
//! - `model` … デバイス設定のデータ型 (UI ↔ ドライバの共通言語, 契約 v2)
//! - `backend` … `HubBackend` / `DeviceDriver` トレイト (バックエンド契約)
//! - `keys` … HID usage ⇔ 名前の表
//! - `registry` … 既知デバイス対応表 + 物理レイアウト
//! - `demo` … 実機なしで全画面を動かすデモバックエンド (caps の書き方の見本)

pub mod backend;
pub mod demo;
pub mod keys;
pub mod lamzu;
pub mod logitech;
pub mod model;
pub mod peq;
pub mod registry;

pub use backend::{DeviceDriver, HubBackend, HubError, HubEvent, HubResult};
pub use model::*;
