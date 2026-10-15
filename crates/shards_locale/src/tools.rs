//! Tools: `locale` and `text`. An agent sees which languages the game has, which one is played
//! and what each lacks, and reads any message in any language with its parameters.

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::text::{Args, Locale};

/// How many missing ids a language's line names before it counts the rest.
const MISSING_SHOWN: usize = 12;

pub(crate) fn register(world: &mut World) {
    register_tool(world, "locale", Scope::Player, ToolKind::Query, locale);
    register_tool(world, "text", Scope::Player, ToolKind::Query, text);
}

/// The game's languages: the one played and the ones its missing messages come from, how many
/// messages each has and which it lacks of the others'. With `language`, every message of that
/// language as it reads (`{$count}` where a parameter goes). Switch the language with
/// `settings_set group=locale key=language value=<id>`.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LocaleArgs {
    /// List this language's messages and their text (`ru`).
    pub language: Option<String>,
}

fn locale(In(call): In<Call<LocaleArgs>>, world: &World) -> ToolResult {
    let locale = world.get_resource::<Locale>().ok_or("this game has no localization module")?;
    if locale.languages().next().is_none() {
        return Ok("no languages: content `*.locale.ron` names them, and locale/<language>/*.ftl holds their text\n".into());
    }
    if let Some(lang) = call.args.language {
        let Some(language) = locale.get(&lang) else {
            let all: Vec<&str> = locale.languages().map(|l| l.id.as_str()).collect();
            let hint = crate::def::closest_language(&lang, all.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            return Err(format!("no language `{lang}`{hint}; languages: {}", all.join(", ")));
        };
        let mut out = format!("{} ({}): {} messages\n", language.id, language.name, language.ids.len());
        for (id, file) in &language.ids {
            let text = locale.plain(&lang, id).unwrap_or_default();
            out.push_str(&format!("{id} = {} [{file}]\n", text.replace('\n', "\\n")));
        }
        return Ok(out);
    }
    let chain = locale.chain();
    let mut out = format!("playing: {}", chain.first().map_or("none", String::as_str));
    if chain.len() > 1 {
        out.push_str(&format!(" (missing messages from {})", chain[1..].join(", ")));
    }
    out.push('\n');
    let all: std::collections::BTreeSet<&str> = locale.languages().flat_map(|l| l.ids.keys().map(String::as_str)).collect();
    for language in locale.languages() {
        out.push_str(&format!("{} {}", language.id, language.name));
        if let Some(fallback) = &language.fallback {
            out.push_str(&format!(" → {fallback}"));
        }
        out.push_str(&format!(": {} messages", language.ids.len()));
        let missing: Vec<&str> = all.iter().copied().filter(|id| !language.ids.contains_key(*id)).collect();
        if !missing.is_empty() {
            let shown = missing.iter().take(MISSING_SHOWN).copied().collect::<Vec<_>>().join(", ");
            let more = missing.len().saturating_sub(MISSING_SHOWN);
            let more = if more > 0 { format!(" and {more} more") } else { String::new() };
            out.push_str(&format!("; lacks {}: {shown}{more}", missing.len()));
        }
        out.push('\n');
    }
    out.push_str("# switch: settings_set group=locale key=language value=<id>; `locale language=<id>` lists one language's text\n");
    Ok(out)
}

/// A message in the language played (or `language`) with its parameters, as the game shows it.
/// A number picks the plural form; `"@id"` puts another message in whole (`{"place":
/// "@place-yard"}`); `"@@x"` is the text `@x`.
#[derive(Deserialize, JsonSchema)]
pub struct TextArgs {
    /// The message: `menu-resume`, or an attribute: `menu-resume.tooltip`.
    pub id: String,
    /// Its parameters as a JSON object: `{"count": 3, "name": "Ada"}`.
    pub args: Option<Value>,
    /// Another language than the one played (`ru`); its fallbacks still apply.
    pub language: Option<String>,
}

fn text(In(call): In<Call<TextArgs>>, world: &World) -> ToolResult {
    let locale = world.get_resource::<Locale>().ok_or("this game has no localization module")?;
    let args = match &call.args.args {
        Some(value) => Args::from_json(value)?,
        None => Args::new(),
    };
    let id = call.args.id.strip_prefix('@').unwrap_or(&call.args.id);
    let lang = match call.args.language.as_deref().or(locale.language()) {
        Some(lang) => lang.to_owned(),
        None => return Err("no languages: content `*.locale.ron` names them".into()),
    };
    let (text, notes) = locale.text_in(&lang, id, &args)?;
    let mut out = format!("{text}\n");
    for note in notes {
        out.push_str(&format!("# {note}\n"));
    }
    Ok(out)
}
