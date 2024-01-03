use crate::{
    components::icons::Icons,
    modules::settings::BatteryMessage,
    style::{GREEN, RED, TEXT},
};
use iced::{
    futures::{
        stream::{self},
        FutureExt, SinkExt, StreamExt,
    },
    Color, Subscription,
};
use std::time::Duration;
use zbus::{dbus_proxy, zvariant::OwnedObjectPath, Connection, Result};

#[derive(Copy, Clone, Debug)]
pub struct BatteryData {
    pub capacity: i64,
    pub status: BatteryStatus,
}

#[derive(Copy, Clone, Debug)]
pub enum BatteryStatus {
    Charging(Duration),
    Discharging(Duration),
    Full,
}

impl BatteryData {
    pub fn get_color(&self) -> Color {
        match self {
            BatteryData {
                status: BatteryStatus::Charging(_),
                ..
            } => GREEN,
            BatteryData {
                status: BatteryStatus::Discharging(_),
                capacity,
            } if *capacity < 20 => RED,
            _ => TEXT,
        }
    }

    pub fn get_icon(&self) -> Icons {
        match self {
            BatteryData {
                status: BatteryStatus::Charging(_),
                ..
            } => Icons::BatteryCharging,
            BatteryData {
                status: BatteryStatus::Discharging(_),
                capacity,
            } if *capacity < 20 => Icons::Battery0,
            BatteryData {
                status: BatteryStatus::Discharging(_),
                capacity,
            } if *capacity < 40 => Icons::Battery1,
            BatteryData {
                status: BatteryStatus::Discharging(_),
                capacity,
            } if *capacity < 60 => Icons::Battery2,
            BatteryData {
                status: BatteryStatus::Discharging(_),
                capacity,
            } if *capacity < 80 => Icons::Battery3,
            _ => Icons::Battery4,
        }
    }
}

#[dbus_proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    fn enumerate_devices(&self) -> Result<Vec<OwnedObjectPath>>;

    #[dbus_proxy(signal)]
    fn device_added(&self) -> Result<OwnedObjectPath>;
}

#[dbus_proxy(
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower/Device",
    interface = "org.freedesktop.UPower.Device"
)]
trait Device {
    #[dbus_proxy(property, name = "Type")]
    fn device_type(&self) -> Result<u32>;

    #[dbus_proxy(property)]
    fn power_supply(&self) -> Result<bool>;

    #[dbus_proxy(property)]
    fn time_to_empty(&self) -> Result<i64>;

    #[dbus_proxy(property)]
    fn time_to_full(&self) -> Result<i64>;

    #[dbus_proxy(property)]
    fn percentage(&self) -> Result<f64>;

    #[dbus_proxy(property)]
    fn state(&self) -> Result<u32>;
}

pub fn subscription() -> Subscription<BatteryMessage> {
    iced::subscription::channel("battery-listener", 100, |mut output| async move {
        let conn = Connection::system().await.unwrap();
        let upower = UPowerProxy::new(&conn).await.unwrap();

        loop {
            let devices = upower.enumerate_devices().await.unwrap();

            let battery = stream::iter(devices.into_iter())
                .filter_map(|device| {
                    let conn = conn.clone();
                    async move {
                        let device = DeviceProxy::builder(&conn)
                            .path(device)
                            .unwrap()
                            .build()
                            .await
                            .unwrap();
                        let device_type = device.device_type().await.unwrap();
                        let power_supply = device.power_supply().await.unwrap();
                        if device_type == 2 && power_supply {
                            Some(device)
                        } else {
                            None
                        }
                    }
                })
                .collect::<Vec<_>>()
                .await;

            if let Some(battery) = battery.first() {
                let state = battery.state().await.unwrap();
                let state = match state {
                    1 => BatteryStatus::Charging(Duration::from_secs(
                        battery.time_to_full().await.unwrap_or_default() as u64,
                    )),
                    2 => BatteryStatus::Discharging(Duration::from_secs(
                        battery.time_to_empty().await.unwrap_or_default() as u64,
                    )),
                    4 => BatteryStatus::Full,
                    _ => BatteryStatus::Discharging(Duration::from_secs(0)),
                };
                let percentage = battery.percentage().await.unwrap_or_default() as i64;

                let _ = output.feed(BatteryMessage::StatusChanged(state)).await;
                let _ = output
                    .feed(BatteryMessage::PercentageChanged(percentage))
                    .await;
                let _ = output.flush().await;

                loop {
                    let mut state_signal = battery.receive_state_changed().await;
                    let mut percentage_signal = battery.receive_percentage_changed().await;
                    let mut time_to_full_signal = battery.receive_time_to_full_changed().await;
                    let mut time_to_empty_signal = battery.receive_time_to_empty_changed().await;

                    iced::futures::select! {
                        state = state_signal.next().fuse() => {
                            if let Some(state) = state {
                                let value = state.get().await;
                                if let Ok(value) = value {
                                    let status = match value {
                                        1 => BatteryStatus::Charging(Duration::from_secs(battery.time_to_full().await.unwrap_or_default() as u64)),
                                        2 => BatteryStatus::Discharging(Duration::from_secs(battery.time_to_empty().await.unwrap_or_default() as u64)),
                                        4 => BatteryStatus::Full,
                                        _ => BatteryStatus::Discharging(Duration::from_secs(0)),
                                    };
                                    let _ = output.send(BatteryMessage::StatusChanged(status)).await;
                                }
                            }
                        },
                        percentage = percentage_signal.next().fuse() => {
                            if let Some(percentage) = percentage {
                                let value = percentage.get().await;
                                if let Ok(value) = value {
                                    let _ = output.send(BatteryMessage::PercentageChanged(value as i64)).await;
                                }
                            }
                        },
                        time_to_full = time_to_full_signal.next().fuse() => {
                            if let Some(time_to_full) = time_to_full {
                                let value = time_to_full.get().await;
                                if let Ok(value) = value {
                                    let _ = output.send(BatteryMessage::StatusChanged(BatteryStatus::Charging(Duration::from_secs(value as u64)))).await;
                                }
                            }
                        },
                        time_to_empty = time_to_empty_signal.next().fuse() => {
                            if let Some(time_to_empty) = time_to_empty {
                                let value = time_to_empty.get().await;
                                if let Ok(value) = value {
                                    let _ = output.send(BatteryMessage::StatusChanged(BatteryStatus::Discharging(Duration::from_secs(value as u64)))).await;
                                }
                            }
                        },
                    }
                }
            } else {
                let _ = upower.receive_device_added().await;
                println!("upower device added");
            }
        }
    })
}
