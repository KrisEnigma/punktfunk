//! The operator-facing settings: one row per setting, in page order.
//!
//! A row is the single source for a setting's env name, store key, kind,
//! default, and when a change takes effect. [`crate::snapshot`] resolves every
//! row as pin > env > store > default; the web console renders the rows the
//! host serves. Env-only knobs (debug, packaging) never get a row.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    /// Inclusive range. An env value outside it is clamped; a store value is refused.
    Int {
        min: i64,
        max: i64,
        unit: &'static str,
    },
    /// Canonical spellings, lowercase snake_case.
    Enum(&'static [&'static str]),
    Text {
        max_len: usize,
    },
    /// Comma-separated in env, a string array in the store.
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Streaming,
    Video,
    Audio,
    Input,
    Network,
    GameMode,
    Session,
    System,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Group::Streaming => "streaming",
            Group::Video => "video",
            Group::Audio => "audio",
            Group::Input => "input",
            Group::Network => "network",
            Group::GameMode => "game_mode",
            Group::Session => "session",
            Group::System => "system",
        }
    }
}

/// When a changed value reaches the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    Now,
    NextSession,
    Restart,
}

impl Apply {
    pub fn as_str(self) -> &'static str {
        match self {
            Apply::Now => "now",
            Apply::NextSession => "next_session",
            Apply::Restart => "restart",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DefaultValue {
    Bool(bool),
    Int(i64),
    Str(&'static str),
    List(&'static [&'static str]),
    /// Linux, then everything else.
    PerOs(&'static DefaultValue, &'static DefaultValue),
}

impl DefaultValue {
    pub fn to_value(self) -> Value {
        match self {
            DefaultValue::Bool(b) => Value::Bool(b),
            DefaultValue::Int(n) => Value::from(n),
            DefaultValue::Str(s) => Value::from(s),
            DefaultValue::List(l) => Value::from(l.to_vec()),
            DefaultValue::PerOs(linux, other) => {
                if cfg!(target_os = "linux") {
                    linux.to_value()
                } else {
                    other.to_value()
                }
            }
        }
    }
}

/// An older env name. `value: None` reads it with the row's own grammar; `Some(v)` means
/// "set at all" selects `v`, whatever it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alias {
    pub name: &'static str,
    pub value: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct Setting {
    /// Store key and API id.
    pub id: &'static str,
    pub env: &'static str,
    pub kind: Kind,
    pub default: DefaultValue,
    pub group: Group,
    pub advanced: bool,
    pub apply: Apply,
    /// `std::env::consts::OS` values this row applies on; empty = every host.
    pub os: &'static [&'static str],
    /// English label, ≤ 3 words. The console localises by `id` and falls back to this.
    pub title: &'static str,
    /// docs-site page slug under `/docs/`.
    pub docs: &'static str,
    /// Checked in order after `env`.
    pub aliases: &'static [Alias],
    /// Extra env spellings for an enum: `(spelling, canonical)`.
    pub spellings: &'static [(&'static str, &'static str)],
}

impl Setting {
    pub fn available(&self) -> bool {
        self.os.is_empty() || self.os.contains(&std::env::consts::OS)
    }

    /// An env string in this row's grammar. `Ok(None)` for blank: blank is unset.
    pub fn parse_env(&self, raw: &str) -> Result<Option<Value>, String> {
        let s = raw.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let lower = s.to_ascii_lowercase();
        let v = match self.kind {
            Kind::Bool => {
                Value::Bool(parse_bool(&lower).ok_or_else(|| format!("{s:?} is not on or off"))?)
            }
            Kind::Int { min, max, .. } => {
                let n: i64 = s.parse().map_err(|_| format!("{s:?} is not a number"))?;
                Value::from(n.clamp(min, max))
            }
            Kind::Enum(options) => {
                let norm = lower.replace('-', "_");
                let hit = [lower.as_str(), norm.as_str()].into_iter().find_map(|c| {
                    options.iter().find(|o| **o == c).copied().or_else(|| {
                        self.spellings
                            .iter()
                            .find(|(sp, _)| *sp == c)
                            .map(|(_, canon)| *canon)
                    })
                });
                Value::from(
                    hit.ok_or_else(|| format!("{s:?} is not one of {}", options.join("/")))?,
                )
            }
            Kind::Text { max_len } => {
                if s.chars().count() > max_len {
                    return Err(format!("longer than {max_len} characters"));
                }
                Value::from(s)
            }
            Kind::List => Value::from(
                s.split(',')
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>(),
            ),
        };
        Ok(Some(v))
    }

    /// A store or API value. Stricter than env: out of range is refused, not clamped.
    pub fn validate(&self, v: &Value) -> Result<Value, String> {
        match (self.kind, v) {
            (Kind::Bool, Value::Bool(_)) => Ok(v.clone()),
            (Kind::Int { min, max, .. }, Value::Number(n)) => match n.as_i64() {
                Some(i) if (min..=max).contains(&i) => Ok(v.clone()),
                _ => Err(format!("must be a whole number from {min} to {max}")),
            },
            (Kind::Enum(options), Value::String(s)) if options.contains(&s.as_str()) => {
                Ok(v.clone())
            }
            (Kind::Enum(options), _) => Err(format!("must be one of {}", options.join(", "))),
            (Kind::Text { max_len }, Value::String(s)) => {
                if s.trim().chars().count() > max_len {
                    Err(format!("must be at most {max_len} characters"))
                } else {
                    Ok(Value::from(s.trim()))
                }
            }
            (Kind::List, Value::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    match it.as_str().map(str::trim) {
                        Some("") => {}
                        Some(s) if !s.contains(',') => out.push(Value::from(s)),
                        _ => return Err("must be a list of names without commas".into()),
                    }
                }
                Ok(Value::Array(out))
            }
            (Kind::Bool, _) => Err("must be true or false".into()),
            (Kind::Int { .. }, _) => Err("must be a number".into()),
            (Kind::Text { .. }, _) => Err("must be text".into()),
            (Kind::List, _) => Err("must be a list".into()),
        }
    }
}

/// The one boolean grammar for registry rows.
pub fn parse_bool(lower: &str) -> Option<bool> {
    match lower {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

pub fn find(id: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.id == id)
}

const LINUX: &[&str] = &["linux"];
const LINUX_WINDOWS: &[&str] = &["linux", "windows"];

/// Page order. Add a row where it reads best; ids are never reused.
pub static SETTINGS: &[Setting] = &[
    // --- Streaming
    Setting {
        id: "gamestream",
        env: "PUNKTFUNK_GAMESTREAM",
        kind: Kind::Bool,
        default: DefaultValue::Bool(false),
        group: Group::Streaming,
        advanced: false,
        apply: Apply::Restart,
        os: &[],
        title: "GameStream",
        docs: "moonlight",
        aliases: &[],
        spellings: &[],
    },
    Setting {
        id: "webtransport",
        env: "PUNKTFUNK_WEBTRANSPORT",
        kind: Kind::Bool,
        default: DefaultValue::Bool(false),
        group: Group::Streaming,
        advanced: false,
        apply: Apply::Restart,
        os: &[],
        title: "Browser streaming",
        docs: "clients",
        aliases: &[],
        spellings: &[],
    },
    Setting {
        id: "clipboard",
        env: "PUNKTFUNK_CLIPBOARD",
        kind: Kind::Enum(&["off", "text", "files"]),
        default: DefaultValue::Str("off"),
        group: Group::Streaming,
        advanced: false,
        apply: Apply::NextSession,
        os: LINUX_WINDOWS,
        title: "Shared clipboard",
        docs: "clipboard",
        aliases: &[],
        spellings: &[
            ("0", "off"),
            ("false", "off"),
            ("no", "off"),
            ("text_only", "text"),
            ("no_files", "text"),
            ("1", "files"),
            ("on", "files"),
            ("true", "files"),
            ("yes", "files"),
            ("all", "files"),
        ],
    },
    Setting {
        id: "host_name",
        env: "PUNKTFUNK_HOST_NAME",
        kind: Kind::Text { max_len: 63 },
        default: DefaultValue::Str(""),
        group: Group::Streaming,
        advanced: false,
        apply: Apply::Restart,
        os: &[],
        title: "Host name",
        docs: "configuration",
        aliases: &[],
        spellings: &[],
    },
    // --- Video
    Setting {
        id: "ten_bit",
        env: "PUNKTFUNK_10BIT",
        kind: Kind::Bool,
        default: DefaultValue::Bool(true),
        group: Group::Video,
        advanced: false,
        apply: Apply::NextSession,
        os: &[],
        title: "10-bit and HDR",
        docs: "hdr",
        aliases: &[],
        spellings: &[],
    },
    Setting {
        id: "chroma_444",
        env: "PUNKTFUNK_444",
        kind: Kind::Bool,
        default: DefaultValue::Bool(true),
        group: Group::Video,
        advanced: false,
        apply: Apply::NextSession,
        os: &[],
        title: "Full color 4:4:4",
        docs: "configuration",
        aliases: &[],
        spellings: &[],
    },
    Setting {
        id: "max_fps",
        env: "PUNKTFUNK_MAX_FPS",
        kind: Kind::Int {
            min: 0,
            max: 240,
            unit: "fps",
        },
        default: DefaultValue::Int(0),
        group: Group::Video,
        advanced: false,
        apply: Apply::NextSession,
        os: LINUX,
        title: "Game frame limit",
        docs: "gamescope",
        aliases: &[],
        spellings: &[],
    },
    // --- Audio
    Setting {
        id: "audio_output_mode",
        env: "PUNKTFUNK_AUDIO_OUTPUT_MODE",
        kind: Kind::Enum(&["client_only", "host_and_client", "follow_default"]),
        default: DefaultValue::Str("client_only"),
        group: Group::Audio,
        advanced: false,
        apply: Apply::NextSession,
        os: LINUX_WINDOWS,
        title: "Where audio plays",
        docs: "configuration",
        aliases: &[
            // A stale host-audio flag must not override "do not touch my devices".
            Alias {
                name: "PUNKTFUNK_KEEP_DEFAULT",
                value: Some("follow_default"),
            },
            Alias {
                name: "PUNKTFUNK_HOST_AUDIO",
                value: Some("host_and_client"),
            },
        ],
        spellings: &[
            ("client", "client_only"),
            ("both", "host_and_client"),
            ("host", "host_and_client"),
            ("follow", "follow_default"),
        ],
    },
    Setting {
        id: "audio_voice_chat",
        env: "PUNKTFUNK_AUDIO_VOICE_CHAT",
        kind: Kind::Enum(&["stream", "host"]),
        default: DefaultValue::Str("stream"),
        group: Group::Audio,
        advanced: false,
        apply: Apply::NextSession,
        os: LINUX,
        title: "Voice chat",
        docs: "configuration",
        aliases: &[],
        spellings: &[("client", "stream"), ("speakers", "host")],
    },
    Setting {
        id: "audio_voice_apps",
        env: "PUNKTFUNK_AUDIO_VOICE_APPS",
        kind: Kind::List,
        default: DefaultValue::List(crate::DEFAULT_VOICE_APPS),
        group: Group::Audio,
        advanced: false,
        apply: Apply::NextSession,
        os: LINUX,
        title: "Voice chat apps",
        docs: "configuration",
        aliases: &[],
        spellings: &[],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_unique_and_well_formed() {
        let mut ids = std::collections::HashSet::new();
        let mut envs = std::collections::HashSet::new();
        for s in SETTINGS {
            assert!(ids.insert(s.id), "duplicate id {}", s.id);
            assert!(
                s.id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{} is not snake_case",
                s.id
            );
            for name in std::iter::once(s.env).chain(s.aliases.iter().map(|a| a.name)) {
                assert!(name.starts_with("PUNKTFUNK_"), "{name}");
                assert!(envs.insert(name), "env name {name} is used twice");
            }
            assert!(s.title.split_whitespace().count() <= 3, "{} title", s.id);
            // The default must satisfy the row's own validation, or a reset stores junk.
            let d = s.default.to_value();
            assert_eq!(s.validate(&d).as_ref(), Ok(&d), "{} default", s.id);
            if let Kind::Enum(options) = s.kind {
                for (_, canon) in s.spellings {
                    assert!(options.contains(canon), "{} spelling -> {canon}", s.id);
                }
            }
        }
    }

    const DOCS_HEADER: &str = "| Setting | `host.env` | Values | Default | Applies |";

    fn docs_table() -> String {
        let code = |s: &str| format!("`{s}`");
        let mut out = format!("{DOCS_HEADER}\n|---|---|---|---|---|\n");
        for s in SETTINGS {
            let values = match s.kind {
                Kind::Bool => "`on` · `off`".to_string(),
                Kind::Int { min, max, unit } => format!("{min}–{max} {unit}"),
                Kind::Enum(o) => o.iter().map(|x| code(x)).collect::<Vec<_>>().join(" · "),
                Kind::Text { max_len } => format!("text, up to {max_len} characters"),
                Kind::List => "comma list".to_string(),
            };
            let default = match s.default.to_value() {
                Value::Bool(b) => code(if b { "on" } else { "off" }),
                Value::String(t) if t.is_empty() => "—".to_string(),
                Value::Array(_) => "built-in list".to_string(),
                Value::String(t) => code(&t),
                v => code(&v.to_string()),
            };
            let applies = match s.apply {
                Apply::Now => "at once",
                Apply::NextSession => "next session",
                Apply::Restart => "after a restart",
            };
            let only = if s.os.is_empty() {
                String::new()
            } else {
                let names: Vec<_> =
                    s.os.iter()
                        .map(|o| match *o {
                            "linux" => "Linux",
                            "windows" => "Windows",
                            other => other,
                        })
                        .collect();
                format!(" ({})", names.join(", "))
            };
            out += &format!(
                "| {}{only} | `{}` | {values} | {default} | {applies} |\n",
                s.title, s.env
            );
        }
        out
    }

    /// `configuration.md` carries the table this registry renders. `UPDATE_SETTINGS_DOCS=1`
    /// rewrites it in place.
    #[test]
    fn docs_table_is_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs-site/content/docs/configuration.md"
        );
        let doc = std::fs::read_to_string(path).expect("read configuration.md");
        let start = doc
            .find(DOCS_HEADER)
            .expect("configuration.md has the settings table");
        let len = doc[start..]
            .split_inclusive('\n')
            .take_while(|l| l.starts_with('|'))
            .map(str::len)
            .sum::<usize>();
        let want = docs_table();
        if std::env::var_os("UPDATE_SETTINGS_DOCS").is_some() {
            let next = format!("{}{want}{}", &doc[..start], &doc[start + len..]);
            std::fs::write(path, next).expect("write configuration.md");
            return;
        }
        assert_eq!(
            &doc[start..start + len],
            want,
            "the settings table in configuration.md is stale — rerun with UPDATE_SETTINGS_DOCS=1"
        );
    }

    #[test]
    fn bool_grammar_is_one_grammar() {
        let s = find("gamestream").unwrap();
        for on in ["1", "true", "ON", " yes "] {
            assert_eq!(s.parse_env(on), Ok(Some(Value::Bool(true))), "{on:?}");
        }
        for off in ["0", "false", "Off", "no"] {
            assert_eq!(s.parse_env(off), Ok(Some(Value::Bool(false))), "{off:?}");
        }
        assert_eq!(s.parse_env("  "), Ok(None));
        assert!(s.parse_env("maybe").is_err());
    }

    #[test]
    fn env_enum_spellings_and_int_clamp() {
        let clip = find("clipboard").unwrap();
        assert_eq!(clip.parse_env("text-only"), Ok(Some(Value::from("text"))));
        assert_eq!(clip.parse_env("on"), Ok(Some(Value::from("files"))));
        assert_eq!(clip.parse_env("FILES"), Ok(Some(Value::from("files"))));
        assert!(clip.parse_env("sometimes").is_err());
        let mode = find("audio_output_mode").unwrap();
        assert_eq!(
            mode.parse_env("host-and-client"),
            Ok(Some(Value::from("host_and_client")))
        );
        let fps = find("max_fps").unwrap();
        assert_eq!(fps.parse_env("500"), Ok(Some(Value::from(240))));
        assert!(fps.parse_env("sixty").is_err());
    }

    #[test]
    fn store_values_are_validated_not_clamped() {
        let fps = find("max_fps").unwrap();
        assert!(fps.validate(&Value::from(60)).is_ok());
        assert!(fps.validate(&Value::from(500)).is_err());
        assert!(fps.validate(&Value::from("60")).is_err());
        let apps = find("audio_voice_apps").unwrap();
        assert_eq!(
            apps.validate(&serde_json::json!([" discord ", ""])),
            Ok(serde_json::json!(["discord"]))
        );
        assert!(apps.validate(&serde_json::json!(["a,b"])).is_err());
        let name = find("host_name").unwrap();
        assert!(name.validate(&Value::from("x".repeat(64))).is_err());
    }
}
