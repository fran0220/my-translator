//! Device identity is an opaque Windows endpoint ID, never a display name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub default: bool,
    pub available: bool,
    pub state: String,
    pub muted: Option<bool>,
}

pub fn enumerate() -> Result<Vec<DeviceInfo>, String> {
    #[cfg(target_os = "windows")]
    {
        super::wasapi::enumerate()
    }
    #[cfg(not(target_os = "windows"))]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        // CPAL 0.15 does not expose portable persistent IDs. Do not fabricate
        // name-based identities on other platforms; only offer their default.
        Ok(cpal::default_host()
            .default_input_device()
            .map(|device| DeviceInfo {
                id: String::new(),
                name: device.name().unwrap_or_else(|_| "默认麦克风".into()),
                kind: Kind::Input,
                default: true,
                available: true,
                state: "默认输入（此平台不支持稳定 ID 选择）".into(),
                muted: None,
            })
            .into_iter()
            .collect())
    }
}

pub fn resolve(devices: &[DeviceInfo], kind: Kind, selection: &str) -> Result<DeviceInfo, String> {
    devices
        .iter()
        .find(|d| {
            d.kind == kind
                && if selection.is_empty() {
                    d.default
                } else {
                    d.id == selection
                }
        })
        .filter(|d| d.available)
        .cloned()
        .ok_or_else(|| "所选音频设备不可用；请连接该设备后重新开始，不会自动改用其他设备".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_not_name_or_new_default_controls_selection() {
        let a = DeviceInfo {
            id: "a".into(),
            name: "USB Audio".into(),
            kind: Kind::Input,
            default: false,
            available: true,
            state: "可用".into(),
            muted: None,
        };
        let b = DeviceInfo {
            id: "b".into(),
            default: true,
            ..a.clone()
        };
        assert_eq!(
            resolve(&[a.clone(), b.clone()], Kind::Input, "a")
                .unwrap()
                .id,
            "a"
        );
        assert_eq!(
            resolve(&[a.clone(), b.clone()], Kind::Input, "")
                .unwrap()
                .id,
            "b"
        );
        assert!(resolve(&[b.clone()], Kind::Input, "a").is_err());
        assert!(resolve(
            &[
                DeviceInfo {
                    available: false,
                    ..a
                },
                b
            ],
            Kind::Input,
            "a"
        )
        .is_err());
    }
}
