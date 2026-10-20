use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        progress::Progress,
        select::{SearchableVec, Select, SelectItem, SelectState},
        *,
    },
    *,
};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use translator_native::{
    audio::{
        devices::{self, DeviceInfo, Kind},
        meter::Meter,
    },
    session::ChannelDiagnostics,
    settings::Settings,
};

#[derive(Clone)]
struct Choice {
    id: String,
    label: String,
    detail: String,
    unavailable: bool,
}
impl SelectItem for Choice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.label.clone().into()
    }
    fn value(&self) -> &String {
        &self.id
    }
    fn disabled(&self) -> bool {
        self.unavailable
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().v_flex().gap_1().child(self.label.clone()).child(
            div()
                .text_xs()
                .text_color(rgb(0x9ca3af))
                .child(self.detail.clone()),
        )
    }
}
type Picker = Entity<SelectState<SearchableVec<Choice>>>;

pub struct AudioPanel {
    input: Picker,
    output: Picker,
    source: Picker,
    devices: Vec<DeviceInfo>,
    updates: mpsc::Receiver<Result<Vec<DeviceInfo>, String>>,
    error: Option<String>,
    pub active: bool,
    pub diagnostics: Option<Arc<Mutex<[ChannelDiagnostics; 2]>>>,
}
impl AudioPanel {
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picker =
            |items: Vec<Choice>, selected: &str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let mut state = SelectState::new(SearchableVec::new(items), None, window, cx)
                        .searchable(true);
                    state.set_selected_value(&selected.to_owned(), window, cx);
                    state
                })
            };
        let input = picker(
            Self::choices(&[], Kind::Input, &settings.input_device_id),
            &settings.input_device_id,
            window,
            cx,
        );
        let output = picker(
            Self::choices(&[], Kind::Output, &settings.output_device_id),
            &settings.output_device_id,
            window,
            cx,
        );
        let source = picker(
            [
                ("microphone", "仅麦克风"),
                ("system", "仅系统回放"),
                ("both", "麦克风 + 系统回放"),
            ]
            .into_iter()
            .map(|(id, label)| Choice {
                id: id.into(),
                label: label.into(),
                detail: String::new(),
                unavailable: false,
            })
            .collect(),
            &settings.audio_source,
            window,
            cx,
        );
        let (tx, updates) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            loop {
                match tx.try_send(devices::enumerate()) {
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                    _ => {}
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            input,
            output,
            source,
            devices: vec![],
            updates,
            error: None,
            active: false,
            diagnostics: None,
        }
    }
    fn choices(devices: &[DeviceInfo], kind: Kind, selected: &str) -> Vec<Choice> {
        let mut choices = vec![Choice {
            id: String::new(),
            label: "系统默认（开始时锁定）".into(),
            detail: "运行中默认设备变化不会切换采集".into(),
            unavailable: false,
        }];
        choices.extend(
            devices
                .iter()
                .filter(|d| d.kind == kind && !d.id.is_empty())
                .map(|d| Choice {
                    id: d.id.clone(),
                    label: format!(
                        "{}{} · {}",
                        d.name,
                        if d.default { " · 默认" } else { "" },
                        d.state
                    ),
                    detail: d.id.clone(),
                    unavailable: !d.available,
                }),
        );
        if !selected.is_empty() && !choices.iter().any(|c| c.id == selected) {
            choices.push(Choice {
                id: selected.into(),
                label: "已选设备离线 · 请重新连接".into(),
                detail: selected.into(),
                unavailable: true,
            });
        }
        choices
    }
    pub fn apply(&self, settings: &mut Settings, cx: &App) {
        settings.input_device_id = self
            .input
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        settings.output_device_id = self
            .output
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        if let Some(source) = self.source.read(cx).selected_value() {
            settings.audio_source = source.clone();
        }
    }
    fn channel(&self, index: usize, cx: &App) -> impl IntoElement {
        let (kind, label, picker) = if index == 0 {
            (Kind::Input, "麦克风 / 输入设备", &self.input)
        } else {
            (Kind::Output, "系统音频 / 回放输出设备", &self.output)
        };
        let selected = picker
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        let channel = self
            .diagnostics
            .as_ref()
            .map(|d| d.lock().unwrap()[index].clone())
            .unwrap_or_default();
        let current = channel.device.as_ref().filter(|_| self.active).or_else(|| {
            self.devices.iter().find(|d| {
                d.kind == kind
                    && if selected.is_empty() {
                        d.default
                    } else {
                        d.id == selected
                    }
            })
        });
        let fresh = self.active
            && channel
                .last_packet
                .is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        let meter = if fresh {
            channel.meter
        } else {
            Meter::default()
        };
        let muted = current
            .and_then(|d| {
                self.devices
                    .iter()
                    .find(|live| live.id == d.id && live.kind == kind)
            })
            .and_then(|d| d.muted)
            == Some(true);
        let status = if muted {
            "设备已静音"
        } else if !self.active || !channel.opened {
            "未采集"
        } else {
            meter.state()
        };
        let color = if meter.peak >= 0.999 {
            rgb(0xf87171)
        } else {
            rgb(0x5eead4)
        };
        let waveform = meter.waveform.clone();
        div()
            .flex_1()
            .min_w_0()
            .v_flex()
            .gap_2()
            .p_4()
            .border_1()
            .border_color(rgb(0x303744))
            .rounded_lg()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(label)
                    .child(div().text_sm().text_color(color).child(status)),
            )
            .child(
                Select::new(picker)
                    .search_placeholder("搜索设备名称…")
                    .disabled(self.active)
                    .w_full(),
            )
            .child(
                div().text_xs().text_color(rgb(0x9ca3af)).child(
                    current
                        .map(|d| {
                            format!(
                                "{} · {}{}\nID: {}",
                                d.name,
                                d.state,
                                if d.default { " · 默认" } else { "" },
                                if d.id.is_empty() {
                                    "平台默认"
                                } else {
                                    &d.id
                                }
                            )
                        })
                        .unwrap_or_else(|| {
                            if selected.is_empty() {
                                "没有可用默认设备；请连接设备（每 2 秒自动刷新）".into()
                            } else {
                                format!(
                                    "已选设备离线 · ID: {selected}\n请重新连接原设备；不会自动切换"
                                )
                            }
                        }),
                ),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let mid = bounds.origin.y + bounds.size.height / 2.;
                        let mut path = PathBuilder::stroke(px(1.));
                        path.move_to(point(bounds.left(), mid));
                        path.line_to(point(bounds.right(), mid));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(0x374151));
                        }
                        for (i, (lo, hi)) in waveform.iter().enumerate() {
                            let x = bounds.left()
                                + bounds.size.width * (i as f32 + 0.5) / waveform.len() as f32;
                            let mut path = PathBuilder::stroke(px(2.));
                            path.move_to(point(x, mid - bounds.size.height * *hi / 2.));
                            path.line_to(point(x, mid - bounds.size.height * *lo / 2. + px(0.5)));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .w_full()
                .flex_shrink_0()
                .h(px(52.)),
            )
            .child(
                Progress::new(("rms", index))
                    .value(((Meter::db(meter.rms) + 60.) / 60. * 100.).max(0.))
                    .color(color)
                    .accessibility_label(format!("{label} RMS 电平")),
            )
            .child(div().text_xs().text_color(rgb(0x9ca3af)).child(format!(
                "RMS {:.1} dBFS    峰值 {:.1} dBFS    {}",
                Meter::db(meter.rms),
                Meter::db(meter.peak),
                if self.active && channel.opened {
                    "设备已打开"
                } else {
                    "设备未打开"
                }
            )))
    }
}
impl Render for AudioPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        while let Ok(result) = self.updates.try_recv() {
            match result {
                Ok(devices) => {
                    self.error = None;
                    if devices != self.devices {
                        self.devices = devices;
                        for (picker, kind) in
                            [(&self.input, Kind::Input), (&self.output, Kind::Output)]
                        {
                            let selected = picker
                                .read(cx)
                                .selected_value()
                                .cloned()
                                .unwrap_or_default();
                            let items = Self::choices(&self.devices, kind, &selected);
                            picker.update(cx, |state, cx| {
                                state.set_items(SearchableVec::new(items), window, cx);
                                state.set_selected_value(&selected, window, cx);
                            });
                        }
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
        div()
            .v_flex()
            .gap_3()
            .child(div().text_lg().child("音频设备与本地试音"))
            .child(
                div().text_sm().text_color(rgb(0x9ca3af)).child(
                    "试音只采集并显示实际 PCM，不播放、不连接翻译 API。停止试音后再开始翻译。",
                ),
            )
            .child(
                Select::new(&self.source)
                    .search_placeholder("搜索音频来源…")
                    .disabled(self.active)
                    .w_full(),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_color(rgb(0xf87171)).child(error))
            })
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_3()
                    .child(self.channel(0, cx))
                    .child(self.channel(1, cx)),
            )
    }
}
