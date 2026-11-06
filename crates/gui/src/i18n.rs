//! GUI translations.
//!
//! Catalogs are standard gettext `.po` files under `po/` at the repo root. They
//! are built into the binary, so a plain `cargo build` binary and the Flatpak
//! carry the same translations with nothing to install.
//!
//! Only the GUI calls [`init`]. Until then, and so in the CLI and in unit tests,
//! every lookup returns the English source text.
//!
//! Placeholders are named, like `{count}`, and are filled by [`fill`]. Keep the
//! lookup functions' arguments string literals: `scripts/update-po.sh` extracts
//! them with xgettext (keywords `tr`, `trc:1c,2` and `ntr:1,2`).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

/// Every shipped catalog: (locale code, `.po` source). The code is matched
/// against the user's language list by [`pick_language`].
const CATALOGS: &[(&str, &str)] = &[
    ("de", include_str!("../../../po/de.po")),
    ("es", include_str!("../../../po/es.po")),
    ("fr", include_str!("../../../po/fr.po")),
    ("it", include_str!("../../../po/it.po")),
    ("ja", include_str!("../../../po/ja.po")),
    ("pl", include_str!("../../../po/pl.po")),
    ("pt_BR", include_str!("../../../po/pt_BR.po")),
    ("ru", include_str!("../../../po/ru.po")),
    ("tr", include_str!("../../../po/tr.po")),
    ("uk", include_str!("../../../po/uk.po")),
    ("zh_CN", include_str!("../../../po/zh_CN.po")),
];

/// gettext joins a context and a msgid with EOT for the lookup key.
const CONTEXT_GLUE: char = '\u{4}';

struct Catalog {
    lang: &'static str,
    entries: HashMap<String, Vec<String>>,
}

static ACTIVE: OnceLock<Option<Catalog>> = OnceLock::new();

/// Chooses the catalog for this process from the user's language settings.
/// Call once, from the GUI path only. Later calls do nothing.
pub fn init() {
    ACTIVE.get_or_init(|| {
        let names: Vec<String> = gtk::glib::language_names()
            .iter()
            .map(|n| n.to_string())
            .collect();
        let lang = pick_language(&names)?;
        let (_, source) = CATALOGS.iter().find(|(code, _)| *code == lang)?;
        Some(Catalog {
            lang,
            entries: parse_po(source),
        })
    });
}

fn active() -> Option<&'static Catalog> {
    ACTIVE.get().and_then(Option::as_ref)
}

/// Translates `msgid`, or returns it unchanged.
pub fn tr(msgid: &'static str) -> &'static str {
    lookup(active(), msgid).unwrap_or(msgid)
}

/// Translates `msgid` in `context`, for a short string whose meaning depends
/// on where it appears.
pub fn trc(context: &'static str, msgid: &'static str) -> &'static str {
    let Some(cat) = active() else { return msgid };
    let key = format!("{context}{CONTEXT_GLUE}{msgid}");
    cat.entries
        .get(&key)
        .and_then(|forms| forms.first())
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .unwrap_or(msgid)
}

/// Translates a message whose form depends on the number `n`.
pub fn ntr(singular: &'static str, plural: &'static str, n: u64) -> &'static str {
    let english = if n == 1 { singular } else { plural };
    let Some(cat) = active() else { return english };
    let Some(forms) = cat.entries.get(singular) else {
        return english;
    };
    forms
        .get(plural_index(cat.lang, n))
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .unwrap_or(english)
}

/// Replaces each `{name}` in `template` with its value.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

fn lookup(cat: Option<&'static Catalog>, msgid: &str) -> Option<&'static str> {
    cat?.entries
        .get(msgid)
        .and_then(|forms| forms.first())
        .filter(|s| !s.is_empty())
        .map(String::as_str)
}

/// Picks the first shipped catalog that matches the user's language list
/// (glib's order: `LANGUAGE`, then `LC_ALL`, `LC_MESSAGES`, `LANG`).
///
/// An exact region match wins (`pt_BR`). A bare language falls back to the one
/// shipped region of it (`pt` → `pt_BR`), except Chinese written in Traditional
/// script (`zh_TW`, `zh_HK`, `zh_MO`), which stays English rather than getting
/// Simplified text. `C` or `POSIX` stops the search: the user asked for English.
fn pick_language(names: &[String]) -> Option<&'static str> {
    // glib lists `zh_TW` and then the bare `zh`; the bare one must not then
    // pick Simplified for a Traditional-script user.
    let mut traditional_chinese = false;
    for name in names {
        let base = name.split(['.', '@']).next().unwrap_or_default();
        if base.is_empty() {
            continue;
        }
        if base == "C" || base == "POSIX" {
            return None;
        }
        if let Some((code, _)) = CATALOGS.iter().find(|(code, _)| *code == base) {
            return Some(code);
        }
        let (lang, region) = match base.split_once('_') {
            Some((l, r)) => (l, Some(r)),
            None => (base, None),
        };
        if lang == "zh" && matches!(region, Some("TW" | "HK" | "MO")) {
            traditional_chinese = true;
            continue;
        }
        if lang == "zh" && region.is_none() && traditional_chinese {
            continue;
        }
        if let Some((code, _)) = CATALOGS
            .iter()
            .find(|(code, _)| code.split('_').next() == Some(lang))
        {
            return Some(code);
        }
    }
    None
}

/// The gettext plural-form index for `n` in `lang`. These match each `.po`
/// header's `Plural-Forms`, which a test checks.
fn plural_index(lang: &str, n: u64) -> usize {
    let (n10, n100) = (n % 10, n % 100);
    let few = (2..=4).contains(&n10) && !(12..=14).contains(&n100);
    match lang {
        "ja" | "zh_CN" => 0,
        "fr" | "pt_BR" => usize::from(n > 1),
        "ru" | "uk" => {
            if n10 == 1 && n100 != 11 {
                0
            } else if few {
                1
            } else {
                2
            }
        }
        "pl" => {
            if n == 1 {
                0
            } else if few {
                1
            } else {
                2
            }
        }
        _ => usize::from(n != 1),
    }
}

/// Parses a `.po` file into lookup keys and their translations. Fuzzy and
/// obsolete entries, and the header, are left out, so they fall back to English.
fn parse_po(source: &str) -> HashMap<String, Vec<String>> {
    #[derive(Default)]
    struct Entry {
        fuzzy: bool,
        ctx: Option<String>,
        id: Option<String>,
        forms: Vec<String>,
    }
    enum Field {
        None,
        Ctx,
        Id,
        IdPlural,
        Str(usize),
    }

    fn finish(entry: Entry, out: &mut HashMap<String, Vec<String>>) {
        let Some(id) = entry.id else { return };
        if entry.fuzzy || id.is_empty() || entry.forms.iter().all(String::is_empty) {
            return;
        }
        let key = match entry.ctx {
            Some(ctx) => format!("{ctx}{CONTEXT_GLUE}{id}"),
            None => id,
        };
        out.insert(key, entry.forms);
    }

    let mut out = HashMap::new();
    let mut entry = Entry::default();
    let mut field = Field::None;
    let mut started = false;

    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(flags) = line.strip_prefix("#,") {
            if started {
                finish(std::mem::take(&mut entry), &mut out);
                started = false;
            }
            entry.fuzzy |= flags.split(',').any(|f| f.trim() == "fuzzy");
            field = Field::None;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (keyword, rest) = match line.split_once(' ') {
            Some((k, r)) if !line.starts_with('"') => (k, r.trim()),
            _ => ("", line),
        };
        let starts_entry =
            matches!(keyword, "msgctxt") || (keyword == "msgid" && !matches!(field, Field::Ctx));
        if starts_entry && started {
            let fuzzy = entry.fuzzy && entry.id.is_none();
            finish(std::mem::take(&mut entry), &mut out);
            entry.fuzzy = fuzzy;
        }
        let text = unquote(rest);
        match keyword {
            "msgctxt" => {
                started = true;
                entry.ctx = Some(text);
                field = Field::Ctx;
            }
            "msgid" => {
                started = true;
                entry.id = Some(text);
                field = Field::Id;
            }
            "msgid_plural" => field = Field::IdPlural,
            "" => match field {
                Field::Ctx => entry.ctx.get_or_insert_with(String::new).push_str(&text),
                Field::Id => entry.id.get_or_insert_with(String::new).push_str(&text),
                Field::Str(i) => {
                    if let Some(s) = entry.forms.get_mut(i) {
                        s.push_str(&text);
                    }
                }
                Field::IdPlural | Field::None => {}
            },
            k if k == "msgstr" || k.starts_with("msgstr[") => {
                let index = k
                    .strip_prefix("msgstr[")
                    .and_then(|r| r.strip_suffix(']'))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                if entry.forms.len() <= index {
                    entry.forms.resize(index + 1, String::new());
                }
                entry.forms[index] = text;
                field = Field::Str(index);
            }
            _ => field = Field::None,
        }
    }
    if started {
        finish(entry, &mut out);
    }
    out
}

/// Strips the quotes from a `.po` string and resolves its escapes.
fn unquote(s: &str) -> String {
    let inner = s
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const TEMPLATE: &str = include_str!("../../../po/appimage-control.pot");

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn catalog(lang: &'static str, po: &str) -> Catalog {
        Catalog {
            lang,
            entries: parse_po(po),
        }
    }

    #[test]
    fn language_choice_follows_the_users_list() {
        assert_eq!(
            pick_language(&names(&["de_DE.UTF-8", "de_DE", "de", "C"])),
            Some("de")
        );
        assert_eq!(
            pick_language(&names(&["pt_BR.UTF-8", "pt_BR", "pt", "C"])),
            Some("pt_BR")
        );
        assert_eq!(pick_language(&names(&["pt_PT", "pt", "C"])), Some("pt_BR"));
        assert_eq!(
            pick_language(&names(&["zh_CN.UTF-8", "zh_CN", "zh", "C"])),
            Some("zh_CN")
        );
        assert_eq!(pick_language(&names(&["uk", "ru", "C"])), Some("uk"));
        assert_eq!(pick_language(&names(&["sr@latin", "uk", "C"])), Some("uk"));
    }

    #[test]
    fn english_and_unshipped_languages_get_no_catalog() {
        assert_eq!(
            pick_language(&names(&["en_US.UTF-8", "en_US", "en", "C"])),
            None
        );
        assert_eq!(pick_language(&names(&["C"])), None);
        assert_eq!(pick_language(&names(&["C", "de"])), None);
        assert_eq!(pick_language(&names(&["nl_NL", "nl", "C"])), None);
    }

    #[test]
    fn traditional_chinese_does_not_get_simplified_text() {
        assert_eq!(
            pick_language(&names(&["zh_TW.UTF-8", "zh_TW", "zh", "C"])),
            None
        );
        assert_eq!(pick_language(&names(&["zh_HK", "zh", "C"])), None);
    }

    #[test]
    fn plural_rules_match_gettext() {
        let pick = |lang: &str, ns: &[u64]| {
            ns.iter()
                .map(|&n| plural_index(lang, n))
                .collect::<Vec<_>>()
        };
        assert_eq!(pick("de", &[0, 1, 2, 5]), [1, 0, 1, 1]);
        assert_eq!(pick("fr", &[0, 1, 2]), [0, 0, 1]);
        assert_eq!(pick("pt_BR", &[0, 1, 2]), [0, 0, 1]);
        assert_eq!(
            pick("ru", &[1, 2, 5, 11, 21, 22, 25, 111, 112]),
            [0, 1, 2, 2, 0, 1, 2, 2, 2]
        );
        assert_eq!(pick("uk", &[1, 3, 14, 101, 104]), [0, 1, 2, 0, 1]);
        assert_eq!(
            pick("pl", &[1, 2, 5, 12, 21, 22, 25]),
            [0, 1, 2, 2, 2, 1, 2]
        );
        assert_eq!(pick("ja", &[0, 1, 2]), [0, 0, 0]);
    }

    #[test]
    fn parser_reads_context_plurals_escapes_and_skips_fuzzy() {
        let po = r#"
msgid ""
msgstr ""
"Plural-Forms: nplurals=2; plural=(n != 1);\n"

#: src/main.rs:1
msgid "Apply"
msgstr "Anwenden"

msgctxt "menu"
msgid "Open"
msgstr "Öffnen"

msgid "{count} instance"
msgid_plural "{count} instances"
msgstr[0] "{count} Instanz"
msgstr[1] "{count} Instanzen"

#, fuzzy
msgid "Old"
msgstr "Alt"

msgid "Two\nlines \"quoted\""
msgstr ""
"Zwei\n"
"Zeilen \"zitiert\""

msgid "Untranslated"
msgstr ""

#~ msgid "Gone"
#~ msgstr "Weg"
"#;
        let entries = parse_po(po);
        assert_eq!(entries["Apply"], ["Anwenden"]);
        assert_eq!(entries["menu\u{4}Open"], ["Öffnen"]);
        assert_eq!(
            entries["{count} instance"],
            ["{count} Instanz", "{count} Instanzen"]
        );
        assert_eq!(
            entries["Two\nlines \"quoted\""],
            ["Zwei\nZeilen \"zitiert\""]
        );
        assert!(
            !entries.contains_key("Old"),
            "fuzzy entries fall back to English"
        );
        assert!(!entries.contains_key("Untranslated"));
        assert!(!entries.contains_key("Gone"));
        assert!(!entries.contains_key(""), "the header is not a message");
    }

    #[test]
    fn lookups_fall_back_to_english() {
        let cat = catalog("de", "msgid \"Apply\"\nmsgstr \"Anwenden\"\n");
        let cat: &'static Catalog = Box::leak(Box::new(cat));
        assert_eq!(lookup(Some(cat), "Apply"), Some("Anwenden"));
        assert_eq!(lookup(Some(cat), "Missing"), None);
        assert_eq!(tr("Apply"), "Apply", "no catalog until init()");
        assert_eq!(ntr("{n} app", "{n} apps", 1), "{n} app");
        assert_eq!(ntr("{n} app", "{n} apps", 3), "{n} apps");
    }

    #[test]
    fn fill_replaces_named_placeholders() {
        assert_eq!(
            fill("{done} of {total}", &[("done", &2), ("total", &5)]),
            "2 of 5"
        );
    }

    /// Placeholders in a message, like `{count}`.
    fn placeholders(s: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(start) = rest.find('{') {
            let after = &rest[start + 1..];
            match after.find('}') {
                Some(end)
                    if after[..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && end > 0 =>
                {
                    out.insert(after[..end].to_string());
                    rest = &after[end + 1..];
                }
                _ => rest = after,
            }
        }
        out
    }

    /// Every message in the template, as its lookup key, with its English
    /// singular and plural text.
    fn template_messages() -> Vec<(String, String, Option<String>)> {
        let mut out = Vec::new();
        let mut ctx: Option<String> = None;
        let mut id: Option<String> = None;
        let mut plural: Option<String> = None;
        let mut field = "";
        let flush = |ctx: &mut Option<String>,
                     id: &mut Option<String>,
                     plural: &mut Option<String>,
                     out: &mut Vec<_>| {
            if let Some(i) = id.take() {
                if !i.is_empty() {
                    let key = match ctx.take() {
                        Some(c) => format!("{c}{CONTEXT_GLUE}{i}"),
                        None => i.clone(),
                    };
                    out.push((key, i, plural.take()));
                }
            }
            *ctx = None;
            *plural = None;
        };
        for line in TEMPLATE.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("msgctxt ") {
                flush(&mut ctx, &mut id, &mut plural, &mut out);
                ctx = Some(unquote(rest));
                field = "ctx";
            } else if let Some(rest) = line.strip_prefix("msgid_plural ") {
                plural = Some(unquote(rest));
                field = "plural";
            } else if let Some(rest) = line.strip_prefix("msgid ") {
                if field != "ctx" {
                    flush(&mut ctx, &mut id, &mut plural, &mut out);
                }
                id = Some(unquote(rest));
                field = "id";
            } else if line.starts_with("msgstr") {
                field = "str";
            } else if line.starts_with('"') {
                let text = unquote(line);
                match field {
                    "ctx" => ctx.get_or_insert_with(String::new).push_str(&text),
                    "id" => id.get_or_insert_with(String::new).push_str(&text),
                    "plural" => plural.get_or_insert_with(String::new).push_str(&text),
                    _ => {}
                }
            }
        }
        flush(&mut ctx, &mut id, &mut plural, &mut out);
        out
    }

    #[test]
    fn template_is_not_empty() {
        assert!(
            template_messages().len() > 100,
            "po/appimage-control.pot looks empty"
        );
    }

    /// Each catalog translates every template message, keeps its placeholders,
    /// has the plural forms its language needs, and declares the plural rule
    /// that `plural_index` uses.
    #[test]
    fn every_catalog_is_complete_and_consistent() {
        let template = template_messages();
        let mut problems = Vec::new();
        for (lang, source) in CATALOGS {
            let nplurals = match *lang {
                "ja" | "zh_CN" => 1,
                "ru" | "uk" | "pl" => 3,
                _ => 2,
            };
            if !source.contains(&format!("nplurals={nplurals};")) {
                problems.push(format!(
                    "{lang}: Plural-Forms header does not say nplurals={nplurals}"
                ));
            }
            let entries = parse_po(source);
            for (key, english, plural) in &template {
                let Some(forms) = entries.get(key) else {
                    problems.push(format!("{lang}: missing or fuzzy: {english:?}"));
                    continue;
                };
                let want = placeholders(english);
                if plural.is_some() {
                    if forms.len() != nplurals || forms.iter().any(String::is_empty) {
                        problems.push(format!(
                            "{lang}: needs {nplurals} plural forms: {english:?}"
                        ));
                    }
                    let plural_want = placeholders(plural.as_deref().unwrap_or_default());
                    for form in forms {
                        let got = placeholders(form);
                        // A form may drop the count where the language says it in words
                        // (Ukrainian "one" form), but never invents a placeholder.
                        if !got.is_subset(&plural_want) {
                            problems.push(format!("{lang}: unknown placeholder in {form:?}"));
                        }
                    }
                    if nplurals == 1 && placeholders(&forms[0]) != plural_want {
                        problems.push(format!("{lang}: placeholders differ in {:?}", forms[0]));
                    }
                } else if placeholders(&forms[0]) != want {
                    problems.push(format!(
                        "{lang}: placeholders differ: {english:?} -> {:?}",
                        forms[0]
                    ));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "{} problem(s):\n{}",
            problems.len(),
            problems.join("\n")
        );
    }
}
