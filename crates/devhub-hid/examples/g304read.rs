//! 読み取り専用: 全候補を IRoot で調べ、応答する G304 の設定を表示する。
//! cargo run -p devhub-hid --example g304read
//! apply / set mode / set profile / memory write は呼ばない。
use devhub_core::model::DriverKind;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hub = devhub_hid::connect()?;
    let mut found = false;
    for info in hub.scan()?.into_iter().filter(|i| i.driver == DriverKind::Logitech) {
        // 同じ VID/PID の受信機が複数あっても、最初の非接続候補で終了しない。
        match hub.driver(&info.id) {
            Ok(driver) => {
                println!("receiver: {}", info.id);
                println!("info: {:?}", driver.info());
                println!("caps: {:?}", driver.caps());
                println!("state: {:#?}", driver.read()?);
                if driver.caps().battery { println!("battery: {:?}", driver.battery()?); }
                found = true;
            }
            Err(error) => eprintln!("{}: {error}", info.id),
        }
    }
    if !found { return Err("応答する G304 がありません（スリープ／電源／受信機を確認）".into()); }
    Ok(())
}
