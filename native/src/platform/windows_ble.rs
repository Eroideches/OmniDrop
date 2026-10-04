//! BLE beacons on Windows 10/11 through WinRT `BluetoothLEAdvertisementPublisher` and
//! `BluetoothLEAdvertisementWatcher`.

use super::BleHandle;
use crate::engine::Core;
use crate::events;
use crate::networking::{self, BLE_COMPANY_ID};
use serde_json::json;
use std::io;
use std::sync::Arc;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementPublisher, BluetoothLEAdvertisementReceivedEventArgs,
    BluetoothLEAdvertisementWatcher, BluetoothLEManufacturerData, BluetoothLEScanningMode,
};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, DataWriter};
use windows::core::Ref;

fn werr(e: windows::core::Error) -> io::Error {
    io::Error::other(format!("Bluetooth: {}", e.message()))
}

fn build_publisher(payload: &[u8]) -> windows::core::Result<BluetoothLEAdvertisementPublisher> {
    let publisher = BluetoothLEAdvertisementPublisher::new()?;
    let data = BluetoothLEManufacturerData::new()?;
    data.SetCompanyId(BLE_COMPANY_ID)?;
    let writer = DataWriter::new()?;
    writer.WriteBytes(payload)?;
    data.SetData(&writer.DetachBuffer()?)?;
    publisher
        .Advertisement()?
        .ManufacturerData()?
        .Append(&data)?;
    Ok(publisher)
}

fn on_advertisement(
    core: &Arc<Core>,
    args: &BluetoothLEAdvertisementReceivedEventArgs,
) -> windows::core::Result<()> {
    let list = args
        .Advertisement()?
        .GetManufacturerDataByCompanyId(BLE_COMPANY_ID)?;
    for i in 0..list.Size()? {
        let data = list.GetAt(i)?.Data()?;
        let reader = DataReader::FromBuffer(&data)?;
        let mut bytes = vec![0u8; reader.UnconsumedBufferLength()? as usize];
        reader.ReadBytes(&mut bytes)?;
        if let Some(beacon) = networking::decode_ble_beacon(&bytes) {
            core.registry
                .observe_ble(&beacon, args.RawSignalStrengthInDBm()?);
        }
    }
    Ok(())
}

pub fn start(core: Arc<Core>) -> io::Result<BleHandle> {
    let publisher = build_publisher(&core.ble_payload()).map_err(werr)?;
    let advertising = match publisher.Start() {
        Ok(()) => true,
        Err(e) => {
            events::log(
                "warn",
                format!("BLE advertising unavailable: {}", e.message()),
            );
            false
        }
    };
    let watcher = BluetoothLEAdvertisementWatcher::new().map_err(werr)?;
    watcher
        .SetScanningMode(BluetoothLEScanningMode::Active)
        .map_err(werr)?;
    let handler_core = core.clone();
    watcher
        .Received(&TypedEventHandler::new(
            move |_sender: Ref<BluetoothLEAdvertisementWatcher>,
                  args: Ref<BluetoothLEAdvertisementReceivedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    let _ = on_advertisement(&handler_core, args);
                }
                Ok(())
            },
        ))
        .map_err(werr)?;
    watcher.Start().map_err(werr)?;
    events::emit(
        "ble_state",
        json!({ "available": true, "active": true, "advertising": advertising }),
    );
    Ok(BleHandle::new(move || {
        let _ = watcher.Stop();
        if advertising {
            let _ = publisher.Stop();
        }
    }))
}
