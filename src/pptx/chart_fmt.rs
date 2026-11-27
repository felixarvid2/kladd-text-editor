//! Talformat för diagrametiketter (Excels formatkoder): `General`, `0`, `0.00`, `#,##0`, procent,
//! exponent, citerad text, avsnitt (`positiv;negativ;noll`) och datum (`m/d/yyyy`, `mmm-yy` …;
//! serienummer från 1899-12-30). Färger (`[Red]`) och språkkoder (`[$-409]`) hoppas över.

/// Formaterar `v` med formatkoden `code`.
pub fn format(v: f64, code: &str) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let sections = split_sections(code);
    let (sec, v, neg_sign) = match sections.len() {
        0 => return general(v),
        1 => (sections[0].as_str(), v, v < 0.0),
        _ if v < 0.0 => (sections[1].as_str(), -v, false),
        n if v == 0.0 && n > 2 => (sections[2].as_str(), v, false),
        _ => (sections[0].as_str(), v, false),
    };
    let sec = strip_brackets(sec);
    let s = if sec.trim().is_empty() || sec.eq_ignore_ascii_case("general") {
        general(v.abs())
    } else if is_date(&sec) {
        return date(v, &sec);
    } else {
        number(v.abs(), &sec)
    };
    if neg_sign && !s.is_empty() && s.chars().any(|c| c.is_ascii_digit() && c != '0') {
        format!("-{s}")
    } else {
        s
    }
}

/// Excels "General": upp till tio värdesiffror, utan nollor på slutet; exponent för mycket stora/små tal.
pub fn general(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-9..1e11).contains(&a) {
        let s = format!("{:.5E}", v);
        // 1.23450E3 → 1.2345E+03
        if let Some((m, e)) = s.split_once('E') {
            let m = m.trim_end_matches('0').trim_end_matches('.');
            let e: i32 = e.parse().unwrap_or(0);
            return format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
        }
        return s;
    }
    let digits = 10 - (a.log10().floor() as i32 + 1).max(0);
    let s = format!("{:.*}", digits.clamp(0, 10) as usize, v);
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

fn split_sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                out.last_mut().unwrap().push(c);
            }
            '\\' => {
                out.last_mut().unwrap().push(c);
                if let Some(n) = chars.next() {
                    out.last_mut().unwrap().push(n);
                }
            }
            ';' if !quoted => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out
}

/// Tar bort `[Red]`, `[$-409]` m.fl. (men behåller valutasymbolen i `[$€-407]`).
fn strip_brackets(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find(']') else {
            rest = "";
            break;
        };
        let inner = &rest[i + 1..i + j];
        if let Some(cur) = inner.strip_prefix('$') {
            let sym = cur.split('-').next().unwrap_or("");
            out.push('"');
            out.push_str(sym);
            out.push('"');
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

fn is_date(s: &str) -> bool {
    let mut quoted = false;
    let mut prev = '\0';
    for c in s.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && prev != '\\' && matches!(c.to_ascii_lowercase(), 'd' | 'm' | 'y' | 'h' | 's') {
            return true;
        }
        prev = c;
    }
    false
}

/// Talformat: siffror (`0`, `#`, `?`), tusental (`,`), decimaler, procent, exponent och text.
fn number(v: f64, sec: &str) -> String {
    let lit_free: String = {
        let mut s = String::new();
        let mut quoted = false;
        let mut esc = false;
        for c in sec.chars() {
            if esc {
                esc = false;
                continue;
            }
            match c {
                '"' => quoted = !quoted,
                '\\' => esc = true,
                _ if !quoted => s.push(c),
                _ => {}
            }
        }
        s
    };
    let pct = lit_free.matches('%').count();
    let v = v * 100f64.powi(pct as i32);
    let exp_pos = lit_free.find(['E', 'e']);
    let num_part = |s: &str| s.chars().filter(|c| matches!(c, '0' | '#' | '?' | '.' | ',')).collect::<String>();
    let mant = num_part(exp_pos.map_or(lit_free.as_str(), |i| &lit_free[..i]));
    let (int_pat, frac_pat) = mant.split_once('.').map_or((mant.as_str(), ""), |(a, b)| (a, b));
    let decimals = frac_pat.chars().filter(|c| matches!(c, '0' | '#' | '?')).count();
    let min_frac = frac_pat.chars().filter(|c| *c == '0').count();
    let thousands = int_pat.trim_end_matches(',').contains(',');
    // Kommatecken sist i heltalsdelen skalar med 1000 per tecken.
    let scale_k = int_pat.len() - int_pat.trim_end_matches(',').len();
    let v = v / 1000f64.powi(scale_k as i32);
    let min_int = int_pat.chars().filter(|c| *c == '0').count();

    let body = if let Some(ei) = exp_pos {
        let exp_digits = lit_free[ei..].chars().filter(|c| *c == '0').count().max(1);
        let e = if v == 0.0 { 0 } else { v.abs().log10().floor() as i32 };
        let m = v / 10f64.powi(e);
        let ms = fixed(m, decimals, min_frac, false, min_int.max(1));
        format!("{ms}E{}{:0w$}", if e < 0 { '-' } else { '+' }, e.abs(), w = exp_digits)
    } else {
        fixed(v, decimals, min_frac, thousands, min_int)
    };
    // Sätt ihop med texten runt sifferdelen.
    let mut out = String::new();
    let mut placed = false;
    let mut quoted = false;
    let mut chars = sec.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                quoted = false;
            } else {
                out.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '_' => {
                chars.next();
                out.push(' ');
            }
            '*' => {
                chars.next();
            }
            '0' | '#' | '?' | '.' | ',' => {
                if !placed {
                    out.push_str(&body);
                    placed = true;
                }
            }
            'E' | 'e' if exp_pos.is_some() => {
                if matches!(chars.peek(), Some('+' | '-')) {
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    if !placed {
        out.push_str(&body);
    }
    out
}

fn fixed(v: f64, decimals: usize, min_frac: usize, thousands: bool, min_int: usize) -> String {
    let s = format!("{:.*}", decimals, v);
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let mut frac = frac.to_owned();
    while frac.len() > min_frac && frac.ends_with('0') {
        frac.pop();
    }
    let mut int = int.trim_start_matches('0').to_owned();
    while int.len() < min_int {
        int.insert(0, '0');
    }
    if thousands && int.len() > 3 {
        let b = int.as_bytes();
        let mut g = String::new();
        for (i, ch) in b.iter().enumerate() {
            if i > 0 && (b.len() - i) % 3 == 0 {
                g.push(',');
            }
            g.push(*ch as char);
        }
        int = g;
    }
    if frac.is_empty() { if int.is_empty() { "0".into() } else { int } } else { format!("{int}.{frac}") }
}

/// Serienummer (dagar sedan 1899-12-30) → år, månad, dag, sekunder in på dagen.
fn civil(serial: f64) -> (i64, u32, u32, f64) {
    let days = serial.floor() as i64;
    let secs = (serial - serial.floor()) * 86400.0;
    // 1899-12-30 är dag 0; räkna om till dagar sedan 1970-01-01 (25569).
    let z = days - 25569 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, secs)
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Saturday", "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday"];

fn date(v: f64, sec: &str) -> String {
    let (y, mo, d, secs) = civil(v);
    let (hh, mi, ss) = ((secs / 3600.0) as u32, ((secs % 3600.0) / 60.0) as u32, (secs % 60.0).round() as u32);
    let ampm = sec.to_ascii_lowercase().contains("am/pm");
    let mut out = String::new();
    let cs: Vec<char> = sec.chars().collect();
    let mut i = 0;
    let mut last_was_hour = false;
    while i < cs.len() {
        let c = cs[i];
        let lc = c.to_ascii_lowercase();
        let run = cs[i..].iter().take_while(|x| x.to_ascii_lowercase() == lc).count();
        match lc {
            '"' => {
                let end = cs[i + 1..].iter().position(|&x| x == '"').map_or(cs.len(), |p| i + 1 + p);
                out.extend(&cs[i + 1..end.min(cs.len())]);
                i = end + 1;
                continue;
            }
            '\\' => {
                if let Some(n) = cs.get(i + 1) {
                    out.push(*n);
                }
                i += 2;
                continue;
            }
            'y' => out.push_str(&if run <= 2 { format!("{:02}", y.rem_euclid(100)) } else { y.to_string() }),
            'd' => out.push_str(&match run {
                1 => d.to_string(),
                2 => format!("{d:02}"),
                3 => DAYS[(v.floor() as i64).rem_euclid(7) as usize][..3].to_owned(),
                _ => DAYS[(v.floor() as i64).rem_euclid(7) as usize].to_owned(),
            }),
            'm' if last_was_hour || cs[i + run..].iter().find(|x| x.is_ascii_alphabetic()).is_some_and(|x| x.to_ascii_lowercase() == 's') && run <= 2 => {
                out.push_str(&if run == 1 { mi.to_string() } else { format!("{mi:02}") })
            }
            'm' => out.push_str(&match run {
                1 => mo.to_string(),
                2 => format!("{mo:02}"),
                3 => MONTHS[(mo as usize + 11) % 12][..3].to_owned(),
                5 => MONTHS[(mo as usize + 11) % 12][..1].to_owned(),
                _ => MONTHS[(mo as usize + 11) % 12].to_owned(),
            }),
            'h' => {
                let h = if ampm { (hh + 11) % 12 + 1 } else { hh };
                out.push_str(&if run == 1 { h.to_string() } else { format!("{h:02}") });
            }
            's' => out.push_str(&if run == 1 { ss.to_string() } else { format!("{ss:02}") }),
            'a' if sec[sec.char_indices().nth(i).map_or(0, |(b, _)| b)..].to_ascii_lowercase().starts_with("am/pm") => {
                out.push_str(if hh < 12 { "AM" } else { "PM" });
                i += 5;
                continue;
            }
            _ => {
                out.push(c);
                i += 1;
                continue;
            }
        }
        last_was_hour = lc == 'h';
        i += run;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allmant_och_tal() {
        assert_eq!(general(4.4000000000000004), "4.4");
        assert_eq!(general(1500.0), "1500");
        assert_eq!(general(0.125), "0.125");
        assert_eq!(format(3.0, "General"), "3");
        assert_eq!(format(0.5, "0%"), "50%");
        assert_eq!(format(0.7123, "0.00%"), "71.23%");
        assert_eq!(format(1234567.891, "#,##0.00"), "1,234,567.89");
        assert_eq!(format(2.5, "0.0"), "2.5");
        assert_eq!(format(-2.5, "0.0"), "-2.5");
        assert_eq!(format(-2.5, "0.0;(0.0)"), "(2.5)");
        assert_eq!(format(1234.0, "\"$\"#,##0"), "$1,234");
        assert_eq!(format(12345.0, "0.00E+00"), "1.23E+04");
        assert_eq!(format(0.0, "#,##0"), "0");
        assert_eq!(format(1.0, "[$€-407]0"), "€1");
    }

    #[test]
    fn datum() {
        // 37377 = 2002-05-01.
        assert_eq!(format(37377.0, "m/d/yyyy"), "5/1/2002");
        assert_eq!(format(37377.0, "d/mm/yyyy"), "1/05/2002");
        assert_eq!(format(37377.0, "mmm-yy"), "May-02");
        assert_eq!(format(37377.0, "yyyy"), "2002");
        assert_eq!(format(37377.5, "h:mm AM/PM"), "12:00 PM");
    }
}
