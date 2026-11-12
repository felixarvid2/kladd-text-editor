//! Kortkommandon: text för menyer och verktygstips. På Mac skrivs Ctrl/Alt/Skift som ⌘/⌥/⇧.

use std::borrow::Cow;

/// Gör om kortkommandon i `text` ("Ctrl+Shift+S", "Fetstil (Ctrl+B)") till platsens skrivsätt.
/// Utanför macOS returneras texten oförändrad.
pub fn shortcut(text: &str) -> Cow<'_, str> {
    shortcut_for(text, cfg!(target_os = "macos"))
}

/// Ren funktion bakom [`shortcut`]; `mac` styr så att båda varianterna kan testas överallt.
pub fn shortcut_for(text: &str, mac: bool) -> Cow<'_, str> {
    if !mac || !(text.contains("Ctrl+") || text.contains("Alt+") || text.contains("Shift+")) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        // Ett kortkommando börjar i början av texten eller efter "(" / blanksteg.
        let at_start = out.is_empty() || out.ends_with(['(', ' ']);
        if at_start {
            if let Some((conv, used)) = convert(rest) {
                out.push_str(&conv);
                rest = &rest[used..];
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    Cow::Owned(out)
}

/// Läser ett kortkommando i början av `s`; ger Mac-texten och antal bytes som förbrukats.
fn convert(s: &str) -> Option<(String, usize)> {
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut rest = s;
    loop {
        if let Some(r) = rest.strip_prefix("Ctrl+") {
            ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Alt+") {
            alt = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Shift+") {
            shift = true;
            rest = r;
        } else {
            break;
        }
    }
    if !(ctrl || alt || shift) || rest.is_empty() {
        return None;
    }
    // Tangenten är ett tecken ("B", "+", "−") eller ett ord ("plus", "klick").
    let end = if rest.chars().next().is_some_and(|c| c.is_alphanumeric()) {
        rest.find(|c: char| !c.is_alphanumeric()).unwrap_or(rest.len())
    } else {
        rest.chars().next().unwrap().len_utf8()
    };
    let key = &rest[..end];
    // Mac-ordning ⌃⌥⇧⌘. Ctrl i våra texter betyder Kommando, så ⌃ används aldrig.
    let mut out = String::new();
    if alt {
        out.push('⌥');
    }
    if shift {
        out.push('⇧');
    }
    if ctrl {
        out.push('⌘');
    }
    if key.chars().count() > 1 {
        out.push('+');
        out.push_str(key);
    } else {
        out.push_str(&key.to_uppercase());
    }
    Some((out, s.len() - rest.len() + end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_symboler_i_macordning() {
        assert_eq!(shortcut_for("Ctrl+Shift+S", true), "⇧⌘S");
        assert_eq!(shortcut_for("Ctrl+Alt+F", true), "⌥⌘F");
        assert_eq!(shortcut_for("Alt+Z", true), "⌥Z");
        assert_eq!(shortcut_for("Ctrl+T", true), "⌘T");
        assert_eq!(shortcut_for("Ctrl++", true), "⌘+");
        assert_eq!(shortcut_for("Ctrl+−", true), "⌘−");
    }

    #[test]
    fn mac_verktygstips() {
        assert_eq!(shortcut_for("Fetstil (Ctrl+B)", true), "Fetstil (⌘B)");
        assert_eq!(shortcut_for("Zooma in (Ctrl+plus)", true), "Zooma in (⌘+plus)");
        assert_eq!(shortcut_for("Gå till fotnot (Ctrl+klick)", true), "Gå till fotnot (⌘+klick)");
        assert_eq!(shortcut_for("Ingen genväg", true), "Ingen genväg");
    }

    #[test]
    fn andra_system_oforandrat() {
        for s in ["Ctrl+Shift+S", "Alt+Z", "Fetstil (Ctrl+B)", "Ctrl++"] {
            assert!(matches!(shortcut_for(s, false), Cow::Borrowed(t) if t == s));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shortcut_pa_mac() {
        assert_eq!(shortcut("Ctrl+O"), "⌘O");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn shortcut_utanfor_mac() {
        assert_eq!(shortcut("Ctrl+O"), "Ctrl+O");
    }
}
