use super::*;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
struct SpeakerEditor {
    speaker_id: String,
    name: String,
    saving: Option<u64>,
    pending_name: Option<Option<String>>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct ConversationLog {
    dates: Vec<String>,
    selected_date: Option<String>,
    entries: Vec<Value>,
    truncated: bool,
    filter: Option<String>,
    loading: Option<u64>,
    scroll_distance: f64,
    scroll_generation: u64,
    scroll_to_end_request: Option<u64>,
    scroll_to_end_generation: u64,
    refresh_pending: bool,
    active: bool,
    load_error: Option<String>,
    speaker_editor: Option<SpeakerEditor>,
    // 選択中の発言は一覧の上限から外れても保持する。再読込で名前入力を作り直さない。
    speaker_details: Option<Value>,
    delete_confirmation: Option<String>,
    delete_in_flight: Option<u64>,
    delete_error: Option<String>,
}

impl ConversationLog {
    // タブを開くたびに読み直し、開いている間に確定した発言も反映する。
    pub(super) fn activate(&mut self, io: &mut PanelIo) {
        self.active = true;
        self.scroll_distance = 0.0;
        if self.loading.is_none() {
            self.loading = Some(io.command("loadLog", json!({"date": self.selected_date})));
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.loading = None;
    }

    // 表示中に受理した snapshot ごとの読み直し。読み込み中の更新は
    // 完了時にもう一度読み直して取りこぼさない。読み込みは常に一件だけで、
    // 古い結果は id の照合で採用しない。
    pub(super) fn refresh(&mut self, io: &mut PanelIo) {
        self.active = true;
        if self.delete_in_flight.is_some() {
            self.refresh_pending = true;
            return;
        }
        if self.loading.is_some() {
            self.refresh_pending = true;
        } else {
            self.refresh_pending = false;
            self.loading = Some(io.command("loadLog", json!({"date": self.selected_date})));
        }
    }

    // 非表示中は新しい読み直しを起こさず、保留だけを破棄する。
    pub(super) fn hidden(&mut self) {
        self.active = false;
        self.refresh_pending = false;
        self.delete_confirmation = None;
    }

    pub(super) fn select_date(&mut self, date: String, io: &mut PanelIo) -> Result<(), String> {
        if !self.dates.iter().any(|known| known == &date) {
            return Err(format!("会話ログの日付が範囲外です: {date}"));
        }
        if self.delete_in_flight.is_some() {
            return Ok(());
        }
        if self.selected_date.as_deref() == Some(date.as_str()) {
            return Ok(());
        }
        self.selected_date = Some(date.clone());
        self.delete_confirmation = None;
        self.delete_error = None;
        // 日付が変わった直後に、前の日の発言詳細を表示し続けない。
        self.speaker_details = None;
        self.speaker_editor = None;
        self.refresh_pending = false;
        self.loading = Some(io.command("loadLog", json!({"date": date})));
        Ok(())
    }

    pub(super) fn request_delete(&mut self) -> Result<(), String> {
        if self.delete_in_flight.is_some() {
            return Ok(());
        }
        let Some(date) = self.selected_date.clone() else {
            return Err("削除する会話ログの日付が選択されていません".to_owned());
        };
        if !self.dates.iter().any(|known| known == &date) {
            return Err(format!("会話ログの日付が範囲外です: {date}"));
        }
        self.delete_error = None;
        self.delete_confirmation = Some(date);
        Ok(())
    }

    pub(super) fn cancel_delete(&mut self) {
        if self.delete_in_flight.is_none() {
            self.delete_confirmation = None;
        }
    }

    pub(super) fn confirm_delete(&mut self, io: &mut PanelIo) -> Result<(), String> {
        if self.delete_in_flight.is_some() {
            return Ok(());
        }
        let Some(date) = self.delete_confirmation.clone() else {
            return Err("会話ログの削除確認がありません".to_owned());
        };
        if self.selected_date.as_deref() != Some(date.as_str())
            || !self.dates.iter().any(|known| known == &date)
        {
            self.delete_confirmation = None;
            return Err("削除対象の日付が変わりました".to_owned());
        }
        self.delete_error = None;
        self.delete_confirmation = None;
        self.delete_in_flight = Some(io.command("deleteLogDay", json!({"date": date})));
        Ok(())
    }

    pub(super) fn set_filter(&mut self, filter: String) -> Result<(), String> {
        let next = match filter.as_str() {
            "all" => None,
            "mic" | "unknown" | "mixed" => Some(filter.clone()),
            tag if crate::commands_speaker::valid_speaker_id(tag) => Some(filter),
            _ => return Err(action_error(&filter)),
        };
        if self.filter != next {
            self.filter = next;
            self.request_scroll_end();
        }
        Ok(())
    }

    pub(super) fn select_speaker(&mut self, speaker_id: String) -> Result<(), String> {
        if !crate::commands_speaker::valid_speaker_id(&speaker_id) {
            return Err(action_error(&speaker_id));
        }
        if self
            .speaker_editor
            .as_ref()
            .is_some_and(|editor| editor.saving.is_some())
        {
            return Ok(());
        }
        if self
            .speaker_editor
            .as_ref()
            .is_some_and(|editor| editor.speaker_id == speaker_id)
        {
            return Ok(());
        }
        let name = self
            .entries
            .iter()
            .find(|entry| Self::is_editable_entry(entry, &speaker_id))
            .and_then(|entry| entry["speakerName"].as_str())
            .unwrap_or_default()
            .to_owned();
        if !self
            .entries
            .iter()
            .any(|entry| Self::is_editable_entry(entry, &speaker_id))
        {
            return Err("名前を編集できる発言が見つかりません".to_owned());
        }
        self.speaker_editor = Some(SpeakerEditor {
            speaker_id,
            name,
            saving: None,
            pending_name: None,
            error: None,
        });
        Ok(())
    }

    pub(super) fn select_entry(&mut self, key: LogEntryKey) -> Result<(), String> {
        if self
            .speaker_editor
            .as_ref()
            .is_some_and(|editor| editor.saving.is_some())
        {
            return Ok(());
        }
        let entry = self
            .entries
            .iter()
            .find(|entry| key.matches(entry) && entry["source"].as_str() == Some("speaker"))
            .cloned()
            .ok_or("話者の詳細を表示する発言が見つかりません")?;
        if self
            .speaker_details
            .as_ref()
            .is_some_and(|selected| key.matches(selected))
        {
            return Ok(());
        }
        let editable = entry["speakerTag"]
            .as_str()
            .filter(|id| Self::is_editable_entry(&entry, id))
            .map(str::to_owned);
        if let Some(id) = editable {
            self.select_speaker(id)?;
        } else {
            self.speaker_editor = None;
        }
        self.speaker_details = Some(entry);
        Ok(())
    }

    pub(super) fn set_speaker_name(&mut self, name: String) {
        if let Some(editor) = self
            .speaker_editor
            .as_mut()
            .filter(|editor| editor.saving.is_none())
        {
            editor.name = name;
            editor.error = None;
        }
    }

    pub(super) fn save_speaker(&mut self, io: &mut PanelIo) -> Result<(), String> {
        let name = self
            .speaker_editor
            .as_ref()
            .map(|editor| editor.name.clone());
        self.enqueue_speaker_rename(name, io)
    }

    pub(super) fn clear_speaker(&mut self, io: &mut PanelIo) -> Result<(), String> {
        self.enqueue_speaker_rename(None, io)
    }

    fn enqueue_speaker_rename(
        &mut self,
        name: Option<String>,
        io: &mut PanelIo,
    ) -> Result<(), String> {
        let Some(editor) = self.speaker_editor.as_mut() else {
            return Err("編集対象の話者が選択されていません".to_owned());
        };
        if editor.saving.is_some() {
            return Ok(());
        }
        editor.error = None;
        let command = io.command(
            "speakerRename",
            json!({"speakerId": editor.speaker_id, "name": name}),
        );
        editor.saving = Some(command);
        editor.pending_name = Some(name);
        Ok(())
    }

    pub(super) fn close_speaker(&mut self) -> Result<(), String> {
        if self
            .speaker_editor
            .as_ref()
            .is_some_and(|editor| editor.saving.is_some())
        {
            return Err("話者名の保存中です".to_owned());
        }
        self.speaker_editor = None;
        self.speaker_details = None;
        Ok(())
    }

    pub(super) fn speaker_rename_completed(&mut self, id: u64, result: IoResult, io: &mut PanelIo) {
        let Some(editor) = self.speaker_editor.as_mut() else {
            return;
        };
        if editor.saving != Some(id) {
            return;
        }
        editor.saving = None;
        if result.ok {
            let saved_name = result.value["speakers"]
                .as_array()
                .and_then(|speakers| {
                    speakers
                        .iter()
                        .find(|speaker| speaker["id"].as_str() == Some(editor.speaker_id.as_str()))
                })
                .and_then(|speaker| speaker["name"].as_str())
                .map(str::to_owned)
                .or_else(|| editor.pending_name.take().flatten())
                .unwrap_or_default();
            editor.pending_name = None;
            editor.name = saved_name;
            editor.error = None;
            self.refresh(io);
        } else {
            editor.pending_name = None;
            editor.error = Some(result.message().to_owned());
        }
    }

    fn is_editable_entry(entry: &Value, speaker_id: &str) -> bool {
        entry["source"].as_str() == Some("speaker")
            && entry["speakerStatus"].as_str() == Some("identified")
            && entry["speakerTag"].as_str() == Some(speaker_id)
    }

    pub(super) fn set_scroll_position(&mut self, position: ScrollPosition) {
        if position.generation < self.scroll_generation {
            return;
        }
        let resumed_following = self.scroll_distance >= 24.0 && position.distance < 24.0;
        self.scroll_distance = position.distance;
        self.scroll_generation = position.generation;
        if resumed_following {
            self.request_scroll_end();
        }
    }

    fn request_scroll_end(&mut self) {
        if self.active && self.scroll_distance < 24.0 {
            let request = self
                .scroll_to_end_request
                .map_or(1, |request| request.saturating_add(1));
            self.scroll_to_end_request = Some(request);
            self.scroll_to_end_generation = self.scroll_generation;
        }
    }

    pub(super) fn loaded(
        &mut self,
        id: u64,
        result: IoResult,
        io: &mut PanelIo,
    ) -> Result<(), String> {
        if self.loading != Some(id) {
            return Ok(());
        }
        self.loading = None;
        if !result.ok {
            self.load_error = Some(result.message().into());
            if self.refresh_pending {
                self.refresh(io);
            }
            return Ok(());
        }
        let value = result.value;
        self.dates = decode(value["dates"].clone())?;
        let selected_date = value["selectedDate"]
            .as_str()
            .ok_or("会話ログの選択日がありません")?
            .to_owned();
        self.selected_date = Some(selected_date.clone());
        if self.delete_confirmation.as_deref().is_some_and(|date| {
            date != selected_date || !self.dates.iter().any(|known| known == date)
        }) {
            self.delete_confirmation = None;
        }
        self.entries = value["entries"]
            .as_array()
            .ok_or("会話ログの発言一覧がありません")?
            .clone();
        if let Some(selected) = &self.speaker_details {
            let key: LogEntryKey = serde_json::from_value(json!({
                "observationId": selected["observationId"], "audioStartMs": selected["audioStartMs"],
                "audioEndMs": selected["audioEndMs"],
            })).map_err(|error| error.to_string())?;
            if let Some(updated) = self
                .entries
                .iter()
                .find(|entry| key.matches(entry))
                .cloned()
            {
                if self.speaker_editor.is_none() {
                    if let Some(id) = updated["speakerTag"]
                        .as_str()
                        .filter(|id| Self::is_editable_entry(&updated, id))
                    {
                        self.select_speaker(id.to_owned())?;
                    }
                }
                self.speaker_details = Some(updated);
            } else {
                // 削除や訂正後に選択中の本文が消えた場合、古い詳細・名前編集を残さない。
                self.speaker_details = None;
                self.speaker_editor = None;
            }
        }
        self.truncated = value["truncated"]
            .as_bool()
            .ok_or("会話ログの truncated がありません")?;
        self.load_error = None;
        self.request_scroll_end();
        if self.refresh_pending {
            self.refresh(io);
        }
        Ok(())
    }

    pub(super) fn delete_completed(&mut self, id: u64, result: IoResult, io: &mut PanelIo) {
        if self.delete_in_flight != Some(id) {
            return;
        }
        self.delete_in_flight = None;
        if result.ok {
            self.delete_error = None;
            self.loading = None;
            self.entries = Vec::new();
            self.truncated = false;
            self.speaker_details = None;
            self.speaker_editor = None;
            if self.active {
                self.refresh(io);
            } else {
                self.refresh_pending = false;
            }
        } else {
            self.delete_error = Some(result.message().to_owned());
            if self.active && self.refresh_pending {
                self.refresh(io);
            }
        }
    }

    fn matches(&self, entry: &Value) -> bool {
        match self.filter.as_deref() {
            None => true,
            Some("mic") => entry["source"].as_str() == Some("mic"),
            Some("unknown") => entry["speakerStatus"].as_str() == Some("unknown"),
            Some("mixed") => entry["speakerStatus"].as_str() == Some("mixed"),
            Some(tag) => entry["speakerTag"].as_str() == Some(tag),
        }
    }

    pub(super) fn view(&self) -> Value {
        let mut speakers = self
            .entries
            .iter()
            .filter_map(|entry| {
                Some(json!({
                    "tag": entry["speakerTag"].as_str()?,
                    "name": entry["speakerName"].as_str(),
                }))
            })
            .collect::<Vec<_>>();
        speakers.sort_by(|left, right| left["tag"].as_str().cmp(&right["tag"].as_str()));
        speakers.dedup_by(|left, right| left["tag"] == right["tag"]);
        let visible = self
            .entries
            .iter()
            .filter(|entry| self.matches(entry))
            .map(Self::entry_view)
            .collect::<Vec<_>>();
        json!({
            "dates": self.dates,
            "selectedDate": self.selected_date,
            "filter": self.filter,
            "speakerDetails": self.speaker_details,
            "speakerEditor": self.speaker_editor.as_ref().map(|editor| json!({
                "speakerId": editor.speaker_id,
                "name": editor.name,
                "saving": editor.saving.is_some(),
                "error": editor.error,
            })),
            "speakers": speakers,
            "entries": visible,
            "scrollToEnd": self.scroll_to_end_request.map(|request| json!({
                "request": request,
                "generation": self.scroll_to_end_generation,
            })),
            "truncated": self.truncated,
            "loading": self.loading.is_some(),
            "loadError": self.load_error,
            "deleteConfirmation": self.delete_confirmation,
            "deleting": self.delete_in_flight.is_some(),
            "deleteError": self.delete_error,
        })
    }

    fn entry_view(entry: &Value) -> Value {
        let mut view = entry.clone();
        let speaker_edit_id = entry["speakerTag"]
            .as_str()
            .filter(|speaker_id| Self::is_editable_entry(entry, speaker_id))
            .map(|speaker_id| Value::String(speaker_id.to_owned()))
            .unwrap_or(Value::Null);
        view.as_object_mut()
            .expect("conversation log entry object")
            .insert("speakerEditId".to_owned(), speaker_edit_id);
        view.as_object_mut().expect("conversation log entry object").insert(
            "speakerDetailsKey".to_owned(),
            if entry["source"].as_str() == Some("speaker") {
                json!({"observationId": entry["observationId"], "audioStartMs": entry["audioStartMs"], "audioEndMs": entry["audioEndMs"]})
            } else { Value::Null },
        );
        view
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LogEntryKey {
    observation_id: String,
    audio_start_ms: Option<u64>,
    audio_end_ms: Option<u64>,
}

impl LogEntryKey {
    fn matches(&self, entry: &Value) -> bool {
        entry["observationId"].as_str() == Some(self.observation_id.as_str())
            && entry["audioStartMs"].as_u64() == self.audio_start_ms
            && entry["audioEndMs"].as_u64() == self.audio_end_ms
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ScrollPosition {
    distance: f64,
    generation: u64,
}
