//! Provider semantics ported from soniox.js, openai-realtime-client.js and
//! qwen-realtime-client.js. Deltas append; snapshots replace; finals are authoritative.
use crate::{openai::OpenAiEvent, qwen::QwenEvent};
use indexmap::IndexMap;
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Segment {
    pub source: String,
    pub target: String,
}

#[derive(Default)]
struct Item {
    text: String,
    final_: bool,
    source_id: String,
}

#[derive(Default)]
pub struct Transcript {
    pub segments: VecDeque<Segment>,
    pub source: String,
    pub target: String,
    pending_sources: VecDeque<String>,
    sources: IndexMap<String, Item>,
    targets: IndexMap<String, Item>,
    completed: VecDeque<String>,
    soniox_source: String,
    soniox_target: String,
}

impl Transcript {
    fn commit(&mut self, source: String, target: String) {
        if source.is_empty() && target.is_empty() {
            return;
        }
        self.segments.push_back(Segment { source, target });
        while self.segments.len() > 1000 {
            self.segments.pop_front();
        }
    }

    pub fn openai(&mut self, event: OpenAiEvent) {
        match event {
            OpenAiEvent::Transcript {
                text,
                is_final: true,
            } => {
                let source = self
                    .pending_sources
                    .pop_front()
                    .unwrap_or_else(|| self.source.clone());
                self.commit(source, text);
                self.source.clear();
                self.target.clear();
            }
            OpenAiEvent::Transcript { text, .. } => self.target.push_str(&text),
            OpenAiEvent::SourceTranscript {
                text,
                is_final: true,
            } => {
                self.pending_sources.push_back(text);
                self.source.clear();
            }
            OpenAiEvent::SourceTranscript { text, .. } => self.source.push_str(&text),
            _ => {}
        }
    }

    pub fn qwen(&mut self, event: QwenEvent) {
        match event {
            QwenEvent::Restarted { .. } => self.finish(),
            QwenEvent::Transcript {
                text,
                is_final: true,
            } => {
                self.commit(String::new(), text);
                self.target.clear();
            }
            QwenEvent::Transcript { text, .. } => self.target = text,
            QwenEvent::Realtime { event } => self.qwen_realtime(event),
            _ => {}
        }
    }

    fn qwen_realtime(&mut self, event: Value) {
        let kind = event["type"].as_str().unwrap_or_default();
        let source_event = matches!(
            kind,
            "conversation.item.input_audio_transcription.delta"
                | "conversation.item.input_audio_transcription.completed"
        );
        let target_event = matches!(kind, "response.text.delta" | "response.text.done");
        let link = kind == "conversation.item.created"
            && event["previous_item_id"].is_string()
            && event["item"]["role"] == "assistant"
            && !event["item"]["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "input_audio"));
        if !source_event && !target_event && !link {
            return;
        }
        let id = if link {
            &event["item"]["id"]
        } else {
            &event["item_id"]
        };
        let Some(id) = id.as_str() else {
            return;
        };
        if self.completed.iter().any(|v| v == id) {
            return;
        }
        let items = if source_event {
            &mut self.sources
        } else {
            &mut self.targets
        };
        let item = items.entry(id.to_owned()).or_default();
        if link {
            item.source_id = event["previous_item_id"]
                .as_str()
                .unwrap_or_default()
                .into();
        } else if kind.ends_with(".delta") {
            if !item.final_ {
                item.text
                    .push_str(event["delta"].as_str().unwrap_or_default());
            }
        } else {
            if let Some(text) = event["transcript"]
                .as_str()
                .or_else(|| event["text"].as_str())
            {
                item.text = text.into();
            }
            item.final_ = true;
        }
        let ready: Vec<_> = self
            .targets
            .iter()
            .filter(|(_, t)| t.final_ && self.sources.get(&t.source_id).is_some_and(|s| s.final_))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            let target = self.targets.shift_remove(&id).unwrap();
            let source = self.sources.shift_remove(&target.source_id).unwrap();
            self.commit(source.text, target.text);
            self.completed.extend([target.source_id, id]);
        }
        while self.completed.len() > 256 {
            self.completed.pop_front();
        }
        self.source = self.sources.values().map(|v| v.text.as_str()).collect();
        self.target = self.targets.values().map(|v| v.text.as_str()).collect();
    }

    pub fn soniox(&mut self, value: Value) {
        let Some(tokens) = value["tokens"].as_array() else {
            return;
        };
        let mut provisional_source = String::new();
        let mut provisional_target = String::new();
        let mut endpoint = false;
        for token in tokens {
            let text = token["text"].as_str().unwrap_or_default();
            if text == "<end>" {
                endpoint = true;
                continue;
            }
            let translated = token["translation_status"] == "translation";
            let final_ = token["is_final"] == true;
            match (translated, final_) {
                (true, true) => self.soniox_target.push_str(text),
                (false, true) => self.soniox_source.push_str(text),
                (true, false) => provisional_target.push_str(text),
                (false, false) => provisional_source.push_str(text),
            }
        }
        self.source = format!("{}{}", self.soniox_source, provisional_source);
        self.target = format!("{}{}", self.soniox_target, provisional_target);
        if endpoint && provisional_target.is_empty() && provisional_source.is_empty() {
            let source = std::mem::take(&mut self.soniox_source);
            let target = std::mem::take(&mut self.soniox_target);
            self.commit(source, target);
            self.source.clear();
            self.target.clear();
        }
    }

    /// Preserve partial text on stop/failover, but never carry provider IDs to a new session.
    pub fn finish(&mut self) {
        if !self.targets.is_empty() || !self.sources.is_empty() {
            for (_, target) in std::mem::take(&mut self.targets) {
                let source = self
                    .sources
                    .shift_remove(&target.source_id)
                    .unwrap_or_default();
                self.commit(source.text, target.text);
            }
            for (_, source) in std::mem::take(&mut self.sources) {
                self.commit(source.text, String::new());
            }
        } else {
            while self.pending_sources.len() > 1 {
                let source = self.pending_sources.pop_front().unwrap();
                self.commit(source, String::new());
            }
            let source = self
                .pending_sources
                .pop_front()
                .unwrap_or_else(|| std::mem::take(&mut self.source));
            let target = std::mem::take(&mut self.target);
            self.commit(source, target);
        }
        self.source.clear();
        self.target.clear();
        self.soniox_source.clear();
        self.soniox_target.clear();
        self.completed.clear();
    }

    pub fn translation(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.target.as_str())
            .filter(|s| !s.is_empty())
            .chain(std::iter::once(self.target.as_str()).filter(|s| !s.is_empty()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn snapshots_replace_deltas_append_and_authoritative_finals_replace() {
        let mut t = Transcript::default();
        t.openai(OpenAiEvent::Transcript {
            text: "你".into(),
            is_final: false,
        });
        t.openai(OpenAiEvent::Transcript {
            text: "好".into(),
            is_final: false,
        });
        assert_eq!(t.target, "你好");
        t.openai(OpenAiEvent::SourceTranscript {
            text: "hello".into(),
            is_final: true,
        });
        t.openai(OpenAiEvent::Transcript {
            text: "您好".into(),
            is_final: true,
        });
        assert_eq!(
            t.segments[0],
            Segment {
                source: "hello".into(),
                target: "您好".into()
            }
        );
        t.qwen(QwenEvent::Transcript {
            text: "世".into(),
            is_final: false,
        });
        t.qwen(QwenEvent::Transcript {
            text: "世界".into(),
            is_final: false,
        });
        assert_eq!(t.target, "世界");
        t.finish();
        t.finish();
        assert_eq!(t.translation(), "您好\n世界");
    }
    #[test]
    fn qwen_out_of_order_links_and_duplicate_finals_preserve_pairs() {
        let mut t = Transcript::default();
        for e in [
            json!({"type":"response.text.done","item_id":"t2","text":"第二"}),
            json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"s1","transcript":"first"}),
            json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"s2","transcript":"second"}),
            json!({"type":"conversation.item.created","previous_item_id":"s2","item":{"id":"t2","role":"assistant"}}),
            json!({"type":"response.text.done","item_id":"t2","text":"duplicate"}),
            json!({"type":"response.text.delta","item_id":"t1","delta":"第"}),
            json!({"type":"conversation.item.created","previous_item_id":"s1","item":{"id":"t1","role":"assistant"}}),
            json!({"type":"response.text.done","item_id":"t1","text":"第一"}),
        ] {
            t.qwen(QwenEvent::Realtime { event: e });
        }
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[0].source, "second");
        assert_eq!(t.segments[1].target, "第一");
        t.finish();
        assert_eq!(t.segments.len(), 2);
        for (id, delta) in [("z-first", "先"), ("a-second", "后")] {
            t.qwen(QwenEvent::Realtime {
                event: json!({"type":"response.text.delta","item_id":id,"delta":delta}),
            });
        }
        assert_eq!(t.target, "先后");
        t.qwen(QwenEvent::Restarted { generation: 1 });
        assert_eq!(t.segments[2].target, "先");
        assert_eq!(t.segments[3].target, "后");
        t.qwen(QwenEvent::Realtime {
            event: json!({"type":"response.text.delta","item_id":"t1","delta":"新会话"}),
        });
        assert_eq!(t.target, "新会话");
        assert_eq!(t.segments.len(), 4);
    }
    #[test]
    fn soniox_provisional_revisions_do_not_duplicate_final_tokens() {
        let mut t = Transcript::default();
        t.soniox(json!({"tokens":[{"text":"你","translation_status":"translation","is_final":true},{"text":"们","translation_status":"translation","is_final":false}]}));
        assert_eq!(t.translation(), "你们");
        t.soniox(json!({"tokens":[{"text":"好","translation_status":"translation","is_final":true},{"text":"<end>","is_final":true}]}));
        assert_eq!(t.translation(), "你好");
        t.finish();
        assert_eq!(t.translation(), "你好");
    }
}
