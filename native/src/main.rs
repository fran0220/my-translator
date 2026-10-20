#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod audio_panel;
use audio_panel::AudioPanel;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        button::*,
        input::{Input, InputState},
        *,
    },
    *,
};
use std::time::Duration;
use translator_native::{
    session::{Event, Session},
    settings::{NativeSettingsStore, Settings},
    transcript::Transcript,
};

struct LiveState {
    transcript: Transcript,
    font_size: f32,
}

struct Translator {
    settings: Settings,
    config_store: Option<NativeSettingsStore>,
    config_error: Option<String>,
    keys: [Entity<InputState>; 3],
    proxy: Entity<InputState>,
    token: Entity<InputState>,
    state: Entity<LiveState>,
    session: Option<Session>,
    broadcast: Option<AnyWindowHandle>,
    status: String,
    settings_open: bool,
    audio_open: bool,
    audio_panel: Entity<AudioPanel>,
    preview: bool,
    subtitle_seen: bool,
    failover: bool,
    stopping: bool,
    replay: std::collections::VecDeque<serde_json::Value>,
    replay_tick: u32,
    offline: bool,
    scroll: ScrollHandle,
}

impl Translator {
    fn new(
        replay: Option<Vec<serde_json::Value>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Offline replay never loads saved credentials or opens a provider connection.
        let (settings, config_store, config_error) = if replay.is_some() {
            (Settings::default(), None, None)
        } else {
            match NativeSettingsStore::open() {
                Ok((store, settings)) => (settings, Some(store), None),
                Err(error) => (Settings::default(), None, Some(error)),
            }
        };
        let input = |value: String, secret: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .masked(secret)
            })
        };
        let keys = [
            settings.soniox_api_key.clone(),
            settings.openai_api_key.clone(),
            settings.qwen_api_key.clone(),
        ]
        .map(|key| input(key, true, window, cx));
        let proxy = input(settings.realtime_proxy_url.clone(), false, window, cx);
        let token = input(settings.realtime_proxy_token.clone(), true, window, cx);
        let state = cx.new(|_| LiveState {
            transcript: Transcript::default(),
            font_size: 36.,
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let offline = replay.is_some();
        let audio_panel = cx.new(|cx| AudioPanel::new(&settings, window, cx));
        let status = if config_error.is_some() {
            "配置不可用 · 请修正后重启"
        } else {
            "就绪 · 原生界面，无需 WebView2"
        }
        .into();
        Self {
            settings,
            config_store,
            config_error,
            keys,
            proxy,
            token,
            state,
            session: None,
            broadcast: None,
            status,
            settings_open: false,
            audio_open: false,
            audio_panel,
            preview: false,
            subtitle_seen: false,
            failover: true,
            stopping: false,
            replay: replay.unwrap_or_default().into(),
            replay_tick: 0,
            offline,
            scroll: ScrollHandle::new(),
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        self.replay_tick += 1;
        if self.replay_tick % 8 == 0 {
            if let Some(event) = self.replay.pop_front() {
                self.status = "离线协议回放 · 未连接服务商".into();
                self.state.update(cx, |state, cx| {
                    state.transcript.soniox(event);
                    cx.notify();
                });
                if !self.settings_open {
                    self.scroll.scroll_to_bottom();
                }
                cx.notify();
            }
        }
        let events: Vec<_> = self
            .session
            .as_ref()
            .map(|s| s.events.try_iter().collect())
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                Event::Status(message) => self.status = message,
                Event::Stopped => {
                    self.session = None;
                    self.stopping = false;
                    self.preview = false;
                    self.audio_panel.update(cx, |panel, cx| {
                        panel.active = false;
                        cx.notify();
                    });
                }
                Event::OpenAi(translator_native::openai::OpenAiEvent::Error {
                    message, ..
                })
                | Event::Qwen(translator_native::qwen::QwenEvent::Error { message, .. }) => {
                    self.status = message
                }
                event => {
                    let before = {
                        let transcript = &self.state.read(cx).transcript;
                        (
                            transcript.source.clone(),
                            transcript.target.clone(),
                            transcript.segments.len(),
                        )
                    };
                    let boundary = matches!(
                        &event,
                        Event::Boundary
                            | Event::Qwen(translator_native::qwen::QwenEvent::Restarted { .. })
                    );
                    self.state.update(cx, |state, cx| {
                        match event {
                            Event::OpenAi(event) => state.transcript.openai(event),
                            Event::Qwen(event) => state.transcript.qwen(event),
                            Event::Soniox(value) => state.transcript.soniox(value),
                            Event::Boundary => state.transcript.finish(),
                            _ => {}
                        }
                        cx.notify();
                    });
                    let transcript = &self.state.read(cx).transcript;
                    if !boundary
                        && before
                            != (
                                transcript.source.clone(),
                                transcript.target.clone(),
                                transcript.segments.len(),
                            )
                    {
                        self.subtitle_seen = true;
                    }
                }
            }
        }
        if !self.settings_open && !self.audio_open {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn save(&mut self, cx: &App) -> Result<(), String> {
        if self.offline {
            return Err("离线回放不保存设置，也不会连接服务商".into());
        }
        if let Some(error) = &self.config_error {
            return Err(error.clone());
        }
        self.audio_panel.read(cx).apply(&mut self.settings, cx);
        self.settings.soniox_api_key = self.keys[0].read(cx).value().to_string();
        self.settings.openai_api_key = self.keys[1].read(cx).value().to_string();
        self.settings.qwen_api_key = self.keys[2].read(cx).value().to_string();
        self.settings.realtime_proxy_url = self.proxy.read(cx).value().trim().to_owned();
        self.settings.realtime_proxy_token = self.token.read(cx).value().trim().to_owned();
        let result = self
            .config_store
            .as_ref()
            .ok_or("原生配置未初始化")?
            .save(&self.settings);
        if let Err(error) = &result {
            self.config_error = Some(error.clone());
        }
        result
    }

    fn toggle(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            session.stop();
            self.stopping = true;
            self.status = "正在停止，等待尾句…".into();
        } else {
            match self
                .save(cx)
                .and_then(|_| Session::start(self.settings.clone(), self.failover))
            {
                Ok(session) => {
                    self.audio_panel.update(cx, |panel, cx| {
                        panel.active = true;
                        panel.diagnostics = Some(session.diagnostics.clone());
                        cx.notify();
                    });
                    self.session = Some(session);
                    self.subtitle_seen = false;
                    self.status = "正在启动采集…".into();
                }
                Err(message) => self.status = message,
            }
        }
        cx.notify();
    }

    fn open_broadcast(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.broadcast {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
        }
        let state = self.state.clone();
        let result = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1000.), px(260.)),
                    cx,
                ))),
                titlebar: None,
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    cx.observe(&state, |_, _, cx| cx.notify()).detach();
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    Broadcast { state, focus }
                })
            },
        );
        match result {
            Ok((handle, _)) => self.broadcast = Some(handle),
            Err(_) => self.status = "无法打开导播窗口".into(),
        }
        cx.notify();
    }

    fn options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.session.is_some();
        div()
            .v_flex()
            .gap_3()
            .child(
                div().h_flex().gap_2().child("翻译服务").children(
                    [("soniox", "Soniox"), ("openai", "OpenAI"), ("qwen", "千问")]
                        .into_iter()
                        .map(|(value, label)| {
                            Button::new(value)
                                .label(label)
                                .selected(self.settings.translation_mode == value)
                                .disabled(
                                    disabled
                                        || (self.settings.translation_type == "two_way"
                                            && value != "soniox"),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.settings.translation_mode = value.into();
                                    cx.notify();
                                }))
                        }),
                ),
            )
            .child(
                div().h_flex().gap_2().child("语言方向").children(
                    [
                        ("en", "英文 → 中文"),
                        ("zh", "中文 → 英文"),
                        ("both", "中英双向 · Soniox"),
                    ]
                    .into_iter()
                    .map(|(value, label)| {
                        let selected = if value == "both" {
                            self.settings.translation_type == "two_way"
                        } else {
                            self.settings.translation_type != "two_way"
                                && self.settings.source_language == value
                        };
                        Button::new(value)
                            .label(label)
                            .selected(selected)
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.translation_type = if value == "both" {
                                    "two_way"
                                } else {
                                    "one_way"
                                }
                                .into();
                                if value == "both" {
                                    this.settings.translation_mode = "soniox".into();
                                    this.settings.language_a = "en".into();
                                    this.settings.language_b = "zh".into();
                                } else {
                                    this.settings.source_language = value.into();
                                    this.settings.target_language =
                                        if value == "en" { "zh" } else { "en" }.into();
                                }
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("failover")
                            .label(if self.failover {
                                "自动故障切换：开"
                            } else {
                                "自动故障切换：关"
                            })
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.failover = !this.failover;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("source-text")
                            .label(if self.settings.show_original {
                                "原文：显示"
                            } else {
                                "原文：隐藏"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.show_original = !this.settings.show_original;
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().v_flex().gap_3()
            .children(["Soniox API 密钥", "OpenAI API 密钥", "千问 API 密钥"].into_iter().enumerate().map(|(index, label)| {
                div().v_flex().gap_1().child(label).child(Input::new(&self.keys[index]))
            }))
            .child(div().h_flex().gap_2().child("千问服务")
                .children([("qianwen", "中国站 · 3.8"), ("dashscope_intl", "新加坡站")].into_iter().map(|(value, label)| {
                    Button::new(value).label(label).selected(self.settings.qwen_service == value).disabled(self.session.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| { this.settings.qwen_service = value.into(); cx.notify(); }))
                })))
            .child("实时中转地址（仅 Soniox / OpenAI；留空直连）").child(Input::new(&self.proxy))
            .child("团队中转令牌").child(Input::new(&self.token))
            .child(div().text_sm().text_color(rgb(0x9ca3af)).child("普通版本不含凭据。密钥在本机设置中保存；请勿分享设置文件。会话中修改的配置在下次开始时生效。"))
            .child(Button::new("save").primary().label("保存设置").disabled(self.offline || self.config_error.is_some()).on_click(cx.listener(|this, _, _, cx| {
                this.status = match this.save(cx) { Ok(()) => "设置已保存".into(), Err(e) => e }; cx.notify();
            })))
    }
}

impl Render for Translator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let mut lines: Vec<_> = state
            .transcript
            .segments
            .iter()
            .rev()
            .take(40)
            .cloned()
            .collect();
        lines.reverse();
        let source = state.transcript.source.clone();
        let target = state.transcript.target.clone();
        div()
            .size_full()
            .v_flex()
            .bg(rgb(0x111318))
            .text_color(rgb(0xf3f4f6))
            .p_6()
            .gap_4()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(div().text_2xl().child("实时翻译"))
                    .child(
                        Button::new("audio-devices")
                            .label(if self.audio_open {
                                "返回字幕"
                            } else {
                                "音频设备 / 试音"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audio_open = !this.audio_open;
                                this.settings_open = false;
                                this.scroll.set_offset(point(px(0.), px(0.)));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("settings")
                            .label(if self.settings_open {
                                "返回字幕"
                            } else {
                                "设置"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_open = !this.settings_open;
                                this.audio_open = false;
                                if this.settings_open {
                                    this.scroll.set_offset(point(px(0.), px(0.)));
                                } else {
                                    this.scroll.scroll_to_bottom();
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(!self.settings_open && !self.audio_open, |view| {
                view.child(self.options(cx))
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("start")
                            .primary()
                            .label(if self.stopping {
                                "正在停止…"
                            } else if self.session.is_some() && !self.preview {
                                "停止翻译"
                            } else {
                                "开始翻译"
                            })
                            .disabled(
                                self.stopping
                                    || (self.preview && self.session.is_some())
                                    || (self.session.is_none()
                                        && (self.offline || self.config_error.is_some())),
                            )
                            .on_click(cx.listener(Self::toggle)),
                    )
                    .child(
                        Button::new("broadcast")
                            .label("打开导播窗口")
                            .on_click(cx.listener(Self::open_broadcast)),
                    ),
            )
            .when(self.audio_open, |view| {
                view.child(
                    div()
                        .h_flex()
                        .gap_3()
                        .child(
                            Button::new("preview")
                                .label(if self.preview {
                                    "停止试音"
                                } else {
                                    "开始本地试音"
                                })
                                .disabled(
                                    self.offline
                                        || self.config_error.is_some()
                                        || self.stopping
                                        || (self.session.is_some() && !self.preview),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(session) = &this.session {
                                        session.stop();
                                        this.stopping = true;
                                    } else if let Err(error) = this.save(cx) {
                                        this.status = error;
                                    } else {
                                        let session = Session::preview(this.settings.clone());
                                        this.audio_panel.update(cx, |panel, cx| {
                                            panel.active = true;
                                            panel.diagnostics = Some(session.diagnostics.clone());
                                            cx.notify();
                                        });
                                        this.session = Some(session);
                                        this.preview = true;
                                        this.subtitle_seen = false;
                                        this.status = "正在打开试音设备 · 不连接服务商".into();
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("save-audio")
                                .label("保存设备选择")
                                .disabled(
                                    self.offline
                                        || self.config_error.is_some()
                                        || self.session.is_some(),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.status = match this.save(cx) {
                                        Ok(()) => "设备选择已保存".into(),
                                        Err(error) => error,
                                    };
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa5b4fc))
                    .child(self.status.clone()),
            )
            .when(self.session.is_some() && !self.preview, |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x9ca3af))
                        .child(if self.subtitle_seen {
                            "字幕：已返回"
                        } else {
                            "字幕：等待服务端返回（设备打开不等于已收到字幕）"
                        }),
                )
            })
            .when_some(self.config_error.clone(), |view, error| {
                view.child(
                    div()
                        .text_color(rgb(0xfca5a5))
                        .child(format!("配置错误：{error}。开始与保存已禁用。")),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("content")
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(if self.audio_open {
                        self.audio_panel.clone().into_any_element()
                    } else if self.settings_open {
                        self.render_settings(cx).into_any_element()
                    } else {
                        div()
                            .v_flex()
                            .gap_4()
                            .children(lines.into_iter().map(|line| {
                                div()
                                    .v_flex()
                                    .gap_1()
                                    .when(self.settings.show_original, |d| {
                                        d.child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0x9ca3af))
                                                .child(line.source),
                                        )
                                    })
                                    .child(div().text_lg().child(line.target))
                            }))
                            .child(
                                div()
                                    .v_flex()
                                    .gap_1()
                                    .when(self.settings.show_original, |d| {
                                        d.child(
                                            div().text_sm().text_color(rgb(0x9ca3af)).child(source),
                                        )
                                    })
                                    .child(div().text_lg().text_color(rgb(0xa5b4fc)).child(target)),
                            )
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child("导播字号")
                    .child(Button::new("smaller").label("−").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.font_size = (s.font_size - 4.).max(16.);
                                cx.notify();
                            });
                        },
                    )))
                    .child(Button::new("larger").label("＋").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.font_size = (s.font_size + 4.).min(120.);
                                cx.notify();
                            });
                        },
                    )))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x9ca3af))
                            .child("纯黑底白字 · 仅译文 · 最新最多 3 视觉行"),
                    ),
            )
    }
}

struct Broadcast {
    state: Entity<LiveState>,
    focus: FocusHandle,
}
impl Render for Broadcast {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        // Earlier logical lines cannot appear among the last three visual lines.
        // Keep wrapping the complete final lines, including arbitrarily long sentences.
        let translated = state.transcript.translation();
        let tail: Vec<_> = translated.lines().rev().take(3).collect();
        let text: SharedString = tail.into_iter().rev().collect::<Vec<_>>().join("\n").into();
        let font_size = state.font_size;
        div()
            .id("broadcast")
            .track_focus(&self.focus)
            .size_full()
            .bg(rgb(0x000000))
            .p_4()
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .on_key_down(|event, window, _| match event.keystroke.key.as_str() {
                "escape" => window.remove_window(),
                "f11" => window.toggle_fullscreen(),
                _ => {}
            })
            .child(
                canvas(
                    move |bounds, window, _| {
                        let run = TextRun {
                            len: text.len(),
                            font: font("sans-serif"),
                            color: rgb(0xffffff).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        window
                            .text_system()
                            .shape_text(text, px(font_size), &[run], Some(bounds.size.width), None)
                            .unwrap_or_default()
                    },
                    move |bounds, lines, window, cx| {
                        let line_height = px(font_size * 1.4);
                        let count: usize = lines
                            .iter()
                            .map(|line| line.wrap_boundaries.len() + 1)
                            .sum();
                        let visible =
                            3usize.min((bounds.size.height / line_height).floor().max(0.) as usize);
                        let height = line_height * visible as f32;
                        let clip = Bounds::new(
                            point(bounds.origin.x, bounds.bottom() - height),
                            size(bounds.size.width, height),
                        );
                        let mut origin = point(
                            bounds.origin.x,
                            bounds.bottom() - line_height * count as f32,
                        );
                        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                            for line in lines {
                                let _ = line.paint(
                                    origin,
                                    line_height,
                                    TextAlign::Left,
                                    Some(bounds),
                                    window,
                                    cx,
                                );
                                origin.y += line_height * (line.wrap_boundaries.len() + 1) as f32;
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<_> = std::env::args().collect();
    let replay = if args.get(1).is_some_and(|v| v == "--replay") {
        let path = args.get(2).expect("--replay 需要 JSONL 文件路径");
        Some(
            std::fs::read_to_string(path)
                .expect("无法读取回放文件")
                .lines()
                .map(|line| serde_json::from_str(line).expect("无效回放 JSON"))
                .collect(),
        )
    } else {
        None
    };
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(960.), px(820.)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("实时翻译 · GPUI Kit".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Translator::new(replay, window, cx)),
            )
            .expect("无法创建原生窗口");
            cx.activate(true);
        });
}
