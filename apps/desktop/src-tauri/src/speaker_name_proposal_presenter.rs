use crate::bubbles::{BubbleAction, BubbleInteraction, BubbleRecord, BubbleSecretInput};
use coosenpai_core::config::Config;
use coosenpai_core::locale::Locale;
use coosenpai_core::speaker_name_proposals::{
    SpeakerNameEvidenceRole, SpeakerNameProposalSource, StoredSpeakerNameProposal,
};

pub(crate) fn action_id(kind: &str, proposal: &StoredSpeakerNameProposal) -> String {
    format!("speaker-name:{kind}:{}:{}", proposal.id, proposal.version)
}

pub(crate) fn confirmation_record(
    proposal: &StoredSpeakerNameProposal,
    config: &Config,
    conversation_generation: u64,
) -> BubbleRecord {
    let locale = Locale::from_config(&config.ui.language);
    let message = confirmation_message(proposal, locale);
    BubbleRecord {
        id: proposal
            .confirmation_bubble_id
            .clone()
            .unwrap_or_else(|| confirmation_id(proposal)),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message,
        message_kind: "speaker-name-confirmation".to_owned(),
        notification_priority: "info".to_owned(),
        caused_by: proposal.source_user_message_ids.first().cloned(),
        display_name: config.companion.display_name.clone(),
        persona: config.companion.persona.clone(),
        avatar_color: config.ui.avatar_color.clone(),
        conversation_generation,
        persistent: true,
        interaction: Some(confirmation_interaction_for_action(proposal, locale)),
    }
}

pub(crate) fn result_record(
    proposal: &StoredSpeakerNameProposal,
    config: &Config,
    conversation_generation: u64,
    now: &chrono::DateTime<chrono::Utc>,
) -> Option<BubbleRecord> {
    let operation = proposal.operation.as_ref()?;
    if !proposal.should_deliver_undo_card_at(now) {
        return None;
    }
    let locale = Locale::from_config(&config.ui.language);
    let message = conversation_result_message(operation, locale, false);
    Some(BubbleRecord {
        id: proposal
            .confirmation_bubble_id
            .clone()
            .unwrap_or_else(|| result_id(proposal)),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message,
        message_kind: "speaker-name-result".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: proposal.source_user_message_ids.first().cloned(),
        display_name: config.companion.display_name.clone(),
        persona: config.companion.persona.clone(),
        avatar_color: config.ui.avatar_color.clone(),
        conversation_generation,
        persistent: true,
        interaction: Some(BubbleInteraction {
            select: None,
            secret_input: None,
            actions: vec![BubbleAction {
                id: action_id("undo", proposal),
                label: localized(locale, "取り消す", "Undo"),
            }],
            detail: None,
            technical_detail: None,
        }),
    })
}

pub(crate) fn edit_interaction(
    proposal: &StoredSpeakerNameProposal,
    config: &Config,
) -> BubbleInteraction {
    let locale = Locale::from_config(&config.ui.language);
    BubbleInteraction {
        select: None,
        secret_input: Some(BubbleSecretInput {
            label: localized(locale, "登録予定の名前", "Name to register"),
            placeholder: proposal.name.clone(),
            action: action_id("edit-submit", proposal),
            submit_label: localized(locale, "確認", "Review"),
            value: Some(proposal.name.clone()),
        }),
        actions: vec![BubbleAction {
            id: action_id("edit-cancel", proposal),
            label: localized(locale, "戻る", "Back"),
        }],
        detail: None,
        technical_detail: None,
    }
}

pub(crate) fn confirmation_id(proposal: &StoredSpeakerNameProposal) -> String {
    format!("speaker-name-confirm-{}-v{}", proposal.id, proposal.version)
}

pub(crate) fn result_id(proposal: &StoredSpeakerNameProposal) -> String {
    format!("speaker-name-result-{}-v{}", proposal.id, proposal.version)
}

fn confirmation_message(proposal: &StoredSpeakerNameProposal, locale: Locale) -> String {
    let evidence_quote = match proposal.source {
        SpeakerNameProposalSource::UserRequest => &proposal.evidence[0],
        SpeakerNameProposalSource::Inferred => proposal
            .evidence
            .iter()
            .find(|evidence| {
                matches!(
                    evidence.role,
                    Some(
                        SpeakerNameEvidenceRole::Response
                            | SpeakerNameEvidenceRole::SelfIntroduction
                    )
                )
            })
            .unwrap_or(&proposal.evidence[0]),
    };
    let quote = format!("「{}」と言っている方", evidence_quote.quote);
    if proposal.source == SpeakerNameProposalSource::Inferred {
        let pair_count = proposal
            .evidence
            .iter()
            .filter(|evidence| evidence.role == Some(SpeakerNameEvidenceRole::Address))
            .count();
        let citation_count = if pair_count == 1 { 2 } else { 1 };
        let third_party_count = proposal
            .evidence
            .iter()
            .filter(|evidence| evidence.role == Some(SpeakerNameEvidenceRole::ThirdPartyMention))
            .count();
        let basis = if pair_count == 1 {
            localized(
                locale,
                &format!("呼びかけと応答1組（引用{citation_count}件）"),
                &format!("one address-response pair ({citation_count} quotations)"),
            )
        } else {
            localized(
                locale,
                &format!("自己紹介の引用{citation_count}件"),
                &format!("{citation_count} self-introduction quotation(s)"),
            )
        };
        let supplemental = if third_party_count > 0 {
            localized(
                locale,
                &format!("、補助の第三者言及{third_party_count}件"),
                &format!(", plus {third_party_count} supplemental third-party mention(s)"),
            )
        } else {
            String::new()
        };
        return match locale {
            Locale::Ja => format!(
                "{quote}は、たぶん「{}」という名前だと思われます。根拠は{basis}{supplemental}です。表示名を「{}」として登録してよいですか？",
                proposal.name,
                proposal.name
            ),
            Locale::En => format!(
                "The person saying “{}” is probably named “{}”. The basis is {basis}{supplemental}. Register “{}” as the display name?",
                evidence_quote.quote,
                proposal.name,
                proposal.name
            ),
        };
    }
    match proposal.current_name.as_deref() {
        Some(current_name) => match locale {
            Locale::Ja => format!(
                "{quote}の現在の表示名は「{current_name}」です。「{}」に変更してよいですか？",
                proposal.name
            ),
            Locale::En => format!(
                "The current display name for the person saying “{}” is “{current_name}”. Change it to “{}”?",
                proposal.evidence[0].quote,
                proposal.name
            ),
        },
        None => match locale {
            Locale::Ja => format!(
                "{quote}の名前を「{}」として登録してよいですか？",
                proposal.name
            ),
            Locale::En => format!(
                "Register the person saying “{}” as “{}”?",
                proposal.evidence[0].quote,
                proposal.name
            ),
        },
    }
}

pub(crate) fn confirmation_interaction_for_action(
    proposal: &StoredSpeakerNameProposal,
    locale: Locale,
) -> BubbleInteraction {
    BubbleInteraction {
        select: None,
        secret_input: None,
        actions: vec![
            BubbleAction {
                id: action_id("register", proposal),
                label: localized(locale, "登録", "Register"),
            },
            BubbleAction {
                id: action_id("edit", proposal),
                label: localized(locale, "名前を修正", "Edit name"),
            },
            BubbleAction {
                id: action_id("reject", proposal),
                label: localized(locale, "却下", "Reject"),
            },
        ],
        detail: None,
        technical_detail: None,
    }
}

pub(crate) fn conversation_result_message(
    operation: &coosenpai_core::speaker_name_proposals::SpeakerNameApplyOperation,
    locale: Locale,
    undo: bool,
) -> String {
    if undo {
        return match (locale, operation.before_name.as_deref()) {
            (Locale::Ja, Some(previous)) => format!("話者の表示名を「{previous}」に戻しました。"),
            (Locale::Ja, None) => "話者の表示名を解除しました。".to_owned(),
            (Locale::En, Some(previous)) => {
                format!("Restored the speaker’s display name to “{previous}”.")
            }
            (Locale::En, None) => "Removed the speaker’s display name.".to_owned(),
        };
    }
    match (locale, operation.before_name.as_deref()) {
        (Locale::Ja, Some(previous)) => format!(
            "話者の表示名を「{previous}」から「{}」に変更しました。",
            operation.after_name
        ),
        (Locale::Ja, None) => format!(
            "話者の表示名を「{}」として登録しました。",
            operation.after_name
        ),
        (Locale::En, Some(previous)) => format!(
            "Changed the speaker’s display name from “{previous}” to “{}”.",
            operation.after_name
        ),
        (Locale::En, None) => format!(
            "Registered the speaker’s display name as “{}”.",
            operation.after_name
        ),
    }
}

fn localized(locale: Locale, japanese: &str, english: &str) -> String {
    match locale {
        Locale::Ja => japanese.to_owned(),
        Locale::En => english.to_owned(),
    }
}
