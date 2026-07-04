use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// Prompt policy: which prompts get answered, by whom, and what happens when
// nobody may answer. Layered JSONC config, same as the Node engine:
//   defaults (below)  <  ~/.puppetty/config.json  <  <cwd>/.puppetty/config.json
// First matching rule wins; a rule earlier in the layering order shadows a
// later rule with the same name (disabled:true tombstones a default).

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Rule {
    pub name: Option<String>,
    #[serde(rename = "match")]
    pub pattern: String,
    pub flags: Option<String>,
    pub action: String, // send | enter | forbid | decider | credential
    pub text: Option<String>,
    pub class: Option<String>, // auto | confirm | forbid
    #[serde(rename = "ref")]
    pub cred_ref: Option<String>,
    pub scope: Option<String>, // line (default) | screen
    pub ai: Option<bool>,
    pub describe: Option<String>,
    pub enter: Option<bool>,
    pub decider: Option<String>,
    pub disabled: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OnUnanswered {
    #[serde(rename = "afterSec")]
    pub after_sec: u64,
    #[serde(rename = "do")]
    pub action: String, // cancel | wait
}

impl Default for OnUnanswered {
    fn default() -> Self {
        Self {
            after_sec: 30,
            action: "cancel".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Logging {
    pub enabled: bool,
    #[serde(rename = "retentionDays")]
    pub retention_days: u64,
    #[serde(rename = "maxTotalMB")]
    pub max_total_mb: u64,
}

impl Default for Logging {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 30,
            max_total_mb: 200,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct DeciderDef {
    pub command: Option<String>,
}

pub struct CompiledRule {
    pub rule: Rule,
    pub regex: fancy_regex::Regex,
}

pub struct Policy {
    pub rules: Vec<Rule>, // including disabled, for `config show`
    pub danger_words: Vec<String>,
    /// Who answers when a danger word is visible: "human" (default — never
    /// automated) or "decider" (refer to the LLM with an explicit caution).
    pub on_danger: String,
    pub on_unanswered: OnUnanswered,
    pub deciders: HashMap<String, DeciderDef>,
    pub logging: Logging,
    pub sources: (bool, bool), // (user, project)
    pub compiled: Vec<CompiledRule>,
    pub danger_re: Option<fancy_regex::Regex>,
}

pub struct Match<'a> {
    pub rule: &'a Rule,
    pub class: &'static str, // auto | confirm | forbid | credential
    /// True when the class was escalated to confirm by a danger word (as
    /// opposed to an explicit confirm rule) — the onDanger policy may route
    /// these to the decider instead of a human.
    pub danger: bool,
}

fn default_rules() -> Vec<Rule> {
    let raw = json!([
        { "name": "secrets",
          "match": "(password|passphrase|passcode|secret|api[ _-]?key|token)\\s*[:：]?\\s*$",
          "flags": "i", "action": "forbid" },
        { "name": "yes-no-bracket",
          "match": "[\\[(](y/n|yes/no|y/n/a)[\\])]\\s*[:：?？]?\\s*$",
          "flags": "i", "action": "send", "text": "y" },
        { "name": "yes-no-default",
          "match": "[\\[(](y/N|Y/n|yes/NO|YES/no)[\\])]\\s*[:：?？]?\\s*$",
          "action": "send", "text": "y" },
        { "name": "continue-question",
          "match": "\\b(continue|proceed|install|ok to proceed)\\s*\\??\\s*$",
          "flags": "i", "action": "send", "text": "y" },
        { "name": "press-enter",
          "match": "press\\s+(enter|return|any key)",
          "flags": "i", "action": "enter" },
        { "name": "confirm-word",
          "match": "type\\s+['\"]?(y|yes)['\"]?\\s+to\\s+(confirm|continue|proceed)",
          "flags": "i", "action": "send", "text": "yes", "class": "confirm" },
    ]);
    serde_json::from_value(raw).expect("default rules are valid")
}

fn default_danger_words() -> Vec<String> {
    [
        "\\bdelete\\b",
        "\\bremove\\b",
        "\\boverwrite\\b",
        "\\bforce\\b",
        "rm -rf",
        "reset --hard",
        "git push",
        "irreversible",
        "cannot be undone",
        "\\bpermanently\\b",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// String-aware JSONC: strips // and /* */ comments and trailing commas,
/// then parses as JSON. Faithful port of the Node engine's parseJsonc.
pub fn parse_jsonc(text: &str) -> Result<Value, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_str = false;
    let mut esc = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push('\n');
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if matches!(chars.get(j), Some('}') | Some(']')) {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    serde_json::from_str(&out).map_err(|e| e.to_string())
}

pub fn user_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".puppetty")
        .join("config.json")
}

fn read_config_file(file: &PathBuf) -> Result<Option<Value>, String> {
    if !file.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    parse_jsonc(&text)
        .map(Some)
        .map_err(|e| format!("invalid config {}: {e}", file.display()))
}

/// Compile a JS-style pattern with optional flags. fancy-regex supports the
/// lookaround/backreference syntax user configs may carry over from JS.
pub fn compile_pattern(pattern: &str, flags: &str) -> Result<fancy_regex::Regex, String> {
    let inline: String = flags.chars().filter(|c| "ims".contains(*c)).collect();
    let full = if inline.is_empty() {
        pattern.to_string()
    } else {
        format!("(?{inline}){pattern}")
    };
    fancy_regex::Regex::new(&full).map_err(|e| e.to_string())
}

fn rules_from(cfg: Option<&Value>) -> Result<Vec<Rule>, String> {
    match cfg.and_then(|c| c.get("rules")) {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

pub fn load_policy(cwd: &str) -> Result<Policy, String> {
    let user = read_config_file(&user_config_path())?;
    let project = read_config_file(&PathBuf::from(cwd).join(".puppetty").join("config.json"))?;

    let mut rules = Vec::new();
    rules.extend(rules_from(project.as_ref())?);
    rules.extend(rules_from(user.as_ref())?);
    rules.extend(default_rules());
    // Dedupe by name (fallback: pattern) — earlier layers shadow later ones.
    let mut seen = std::collections::HashSet::new();
    rules.retain(|r| seen.insert(r.name.clone().unwrap_or_else(|| r.pattern.clone())));

    let pick = |key: &str| -> Option<Value> {
        project
            .as_ref()
            .and_then(|p| p.get(key).cloned())
            .or_else(|| user.as_ref().and_then(|u| u.get(key).cloned()))
    };
    let danger_words: Vec<String> = match pick("dangerWords") {
        Some(v) => serde_json::from_value(v).map_err(|e| e.to_string())?,
        None => default_danger_words(),
    };
    let on_danger = match pick("onDanger") {
        Some(v) => match v.as_str() {
            Some(s @ ("human" | "decider")) => s.to_string(),
            _ => return Err("onDanger must be \"human\" or \"decider\"".into()),
        },
        None => "human".to_string(),
    };
    let merge_obj = |key: &str, base: Value| -> Value {
        let mut out = base;
        for layer in [user.as_ref(), project.as_ref()].into_iter().flatten() {
            if let (Some(a), Some(b)) = (
                out.as_object_mut(),
                layer.get(key).and_then(|v| v.as_object()),
            ) {
                for (k, v) in b {
                    a.insert(k.clone(), v.clone());
                }
            }
        }
        out
    };
    let on_unanswered: OnUnanswered = serde_json::from_value(merge_obj(
        "onUnanswered",
        serde_json::to_value(OnUnanswered::default()).unwrap(),
    ))
    .map_err(|e| e.to_string())?;
    let deciders: HashMap<String, DeciderDef> =
        serde_json::from_value(merge_obj("deciders", json!({}))).map_err(|e| e.to_string())?;
    let logging: Logging = serde_json::from_value(merge_obj(
        "logging",
        serde_json::to_value(Logging::default()).unwrap(),
    ))
    .map_err(|e| e.to_string())?;

    let mut compiled = Vec::new();
    for r in rules.iter().filter(|r| r.disabled != Some(true)) {
        let regex = compile_pattern(&r.pattern, r.flags.as_deref().unwrap_or("")).map_err(|e| {
            format!(
                "invalid rule \"{}\": {e}",
                r.name.clone().unwrap_or_else(|| r.pattern.clone())
            )
        })?;
        compiled.push(CompiledRule {
            rule: r.clone(),
            regex,
        });
    }
    let danger_re = if danger_words.is_empty() {
        None
    } else {
        Some(compile_pattern(&danger_words.join("|"), "i")?)
    };

    Ok(Policy {
        rules,
        danger_words,
        on_danger,
        on_unanswered,
        deciders,
        logging,
        sources: (user.is_some(), project.is_some()),
        compiled,
        danger_re,
    })
}

/// First matching rule with its effective severity class, or None. Rules
/// match the prompt line by default, the whole screen with scope:"screen".
/// Danger words anywhere on the visible screen escalate `auto` to `confirm`,
/// but never override an explicit forbid/credential.
pub fn evaluate<'a>(policy: &'a Policy, line: &str, screen: &str) -> Option<Match<'a>> {
    for c in &policy.compiled {
        let target = if c.rule.scope.as_deref() == Some("screen") {
            screen
        } else {
            line
        };
        if !c.regex.is_match(target).unwrap_or(false) {
            continue;
        }
        let mut class: &'static str = if c.rule.action == "credential" {
            "credential"
        } else {
            match c.rule.class.as_deref() {
                Some("confirm") => "confirm",
                Some("forbid") => "forbid",
                Some("auto") => "auto",
                _ => {
                    if c.rule.action == "forbid" {
                        "forbid"
                    } else {
                        "auto"
                    }
                }
            }
        };
        let mut danger = false;
        if class == "auto" {
            if let Some(re) = &policy.danger_re {
                if re.is_match(screen).unwrap_or(false) {
                    class = "confirm";
                    danger = true;
                }
            }
        }
        return Some(Match {
            rule: &c.rule,
            class,
            danger,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        load_policy(".").unwrap()
    }

    #[test]
    fn jsonc_strips_comments_and_trailing_commas() {
        let v = parse_jsonc("{\n // c\n \"a\": [1, 2, /* x */ 3,],\n}").unwrap();
        assert_eq!(v["a"], json!([1, 2, 3]));
    }

    /// The compatibility contract for JS-flavored user patterns, documented
    /// in README.md §"JS regex compatibility". Every construct listed as
    /// supported there must compile through compile_pattern and match (or
    /// not match) exactly as it would in a JS RegExp.
    #[test]
    fn js_regex_compatibility_spec() {
        let supported: &[(&str, &str, &str, bool)] = &[
            // (pattern, flags, input, expect_match)
            ("[y/n]\\)?\\s*$", "i", "Continue? (Y/N)", true), // char class + anchors
            ("\\bforce\\b", "", "use force now", true),       // word boundary
            ("\\bforce\\b", "", "forced", false),
            ("\\d+\\s*%", "", "15 %", true), // digit/space classes
            ("PASSWORD", "i", "password:", true), // i flag
            ("^bar$", "m", "foo\nbar", true), // m flag
            ("a.b", "s", "a\nb", true),      // s flag (dotAll)
            ("a.b", "", "a\nb", false),
            ("foo(?=bar)", "", "foobar", true), // lookahead
            ("foo(?=bar)", "", "foobaz", false),
            ("foo(?!bar)", "", "foobaz", true), // negative lookahead
            ("(?<=\\$)\\d+", "", "$42", true),  // lookbehind
            ("(?<!\\\\)\"", "", "say \"hi", true), // negative lookbehind
            ("(['\"]).*?\\1", "", "'quoted'", true), // backreference
            ("(?<key>\\w+)=", "", "name=", true), // named group
            ("\\u00e9", "", "caf\u{e9}", true), // \uXXXX escape
            ("[:：]\\s*$", "", "パスワード：", true), // unicode literal in class
            ("\\p{L}+", "", "日本語", true),    // unicode property (JS /u)
            ("colou?r", "", "color", true),     // optional
            ("(yes|no|y/n)", "", "reply y/n please", true), // alternation
        ];
        for (pattern, flags, input, expect) in supported {
            let re = compile_pattern(pattern, flags)
                .unwrap_or_else(|e| panic!("pattern {pattern:?} must compile: {e}"));
            assert_eq!(
                re.is_match(input).unwrap(),
                *expect,
                "pattern {pattern:?} flags {flags:?} vs input {input:?}"
            );
        }

        // Documented as UNSUPPORTED: these JS constructs are rejected at
        // compile time (config validate reports them) rather than silently
        // misbehaving. If a fancy-regex upgrade starts accepting one, move
        // it to the supported table and update the README.
        let unsupported: &[&str] = &[
            "\\cJ", // control-character escape
            "[]",   // empty class (JS: never matches; Rust: parse error)
        ];
        for pattern in unsupported {
            assert!(
                compile_pattern(pattern, "").is_err(),
                "pattern {pattern:?} is documented as unsupported and must fail to compile"
            );
        }
    }

    #[test]
    fn default_rules_classify() {
        let p = policy();
        let m = evaluate(
            &p,
            "Overwrite existing file? [y/N]",
            "Overwrite existing file? [y/N]",
        )
        .unwrap();
        assert_eq!(p.on_danger, "human"); // safe default
        assert_eq!(m.class, "confirm"); // danger word "overwrite" escalates
        assert!(m.danger); // ...marked as escalation, not an explicit confirm rule
        let m = evaluate(&p, "Continue? [y/N]", "Continue? [y/N]").unwrap();
        assert_eq!(m.class, "auto");
        let m = evaluate(&p, "Password:", "Password:").unwrap();
        assert_eq!(m.class, "forbid");
    }
}
