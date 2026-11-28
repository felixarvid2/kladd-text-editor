//! MicroType Express (MTX): avkodning av PowerPoints komprimerade inbäddade typsnitt (`.fntdata`,
//! EOT med flaggan `TTEMBED_TTCOMPRESSED`).
//!
//! Följer W3C:s MTX-specifikation (Member Submission 2008-03-05, Monotype): datablock 1 avkomprimeras
//! med LZCOMP (LZ77 med tre adaptiva Huffmanträd och en förladdad historik), och resultatet är ett
//! sfnt i *Compact Table Format* där `glyf` är omkodad och `loca` saknas. Här återskapas `glyf` och
//! `loca` i vanligt TrueType-format. Hintningen (block 2 och 3: pushdata och instruktioner) behövs inte
//! för att rita och tas bort, liksom tabellerna som bara används av hintningen eller som är
//! bitkomprimerade i CTF (`cvt `, `fpgm`, `prep`, `hdmx`, `VDMX`, `LTSH`). Glyfernas konturer,
//! teckenmåtten och formningstabellerna blir exakt som i originalet.
//!
//! Skriven i Rust efter specifikationens beskrivning och dess exempelkod för LZCOMP och AHUFF
//! (W3C Document License); ingen annan avkodare (t.ex. libeot) är använd. Trippelkodningen av punkterna
//! är densamma som i WOFF2.

/// Läser bitar med den mest signifikanta först.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    left: u32,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<bool> {
        if self.left == 0 {
            self.buf = *self.data.get(self.pos)? as u32;
            self.pos += 1;
            self.left = 8;
        }
        self.left -= 1;
        Some(self.buf >> self.left & 1 == 1)
    }

    fn value(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = v << 1 | self.bit()? as u32;
        }
        Some(v)
    }
}

#[derive(Clone, Copy, Default)]
struct Node {
    up: usize,
    left: usize,
    right: usize,
    /// Symbolet för löv, annars −1.
    code: i32,
    weight: i64,
}

const ROOT: usize = 1;

/// Adaptivt Huffmanträd (AHUFF) för symbolerna 0..range.
struct Ahuff {
    tree: Vec<Node>,
    index: Vec<usize>,
}

impl Ahuff {
    fn new(range: usize) -> Ahuff {
        let mut tree = vec![Node::default(); 2 * range];
        for (i, n) in tree.iter_mut().enumerate().skip(2) {
            n.up = i / 2;
            n.weight = 1;
        }
        for (i, n) in tree.iter_mut().enumerate().take(range).skip(1) {
            n.left = 2 * i;
            n.right = 2 * i + 1;
        }
        let mut index = vec![0; range];
        for i in 0..range {
            tree[i].code = -1;
            tree[range + i].code = i as i32;
            index[i] = range + i;
        }
        let mut t = Ahuff { tree, index };
        t.init_weight(ROOT);
        if range > 256 && range < 512 {
            t.update(t.index[256]);
            t.update(t.index[257]);
            for _ in 0..12 {
                t.update(t.index[range - 3]);
            }
            for _ in 0..6 {
                t.update(t.index[range - 2]);
            }
        } else {
            for _ in 0..2 {
                for i in 0..range {
                    t.update(t.index[i]);
                }
            }
        }
        t
    }

    fn init_weight(&mut self, a: usize) -> i64 {
        if self.tree[a].code < 0 {
            let (l, r) = (self.tree[a].left, self.tree[a].right);
            self.tree[a].weight = self.init_weight(l) + self.init_weight(r);
        }
        self.tree[a].weight
    }

    fn swap(&mut self, a: usize, b: usize) {
        let (upa, upb) = (self.tree[a].up, self.tree[b].up);
        self.tree.swap(a, b);
        self.tree[a].up = upa;
        self.tree[b].up = upb;
        for x in [a, b] {
            let n = self.tree[x];
            if n.code < 0 {
                self.tree[n.left].up = x;
                self.tree[n.right].up = x;
            } else {
                self.index[n.code as usize] = x;
            }
        }
    }

    fn update(&mut self, mut a: usize) {
        while a != ROOT {
            let w = self.tree[a].weight;
            let mut b = a - 1;
            if self.tree[b].weight == w {
                while b > ROOT && self.tree[b - 1].weight == w {
                    b -= 1;
                }
                if b > ROOT {
                    self.swap(a, b);
                    a = b;
                }
            }
            self.tree[a].weight = w + 1;
            a = self.tree[a].up;
        }
        self.tree[ROOT].weight += 1;
    }

    fn read(&mut self, bits: &mut Bits<'_>) -> Option<usize> {
        let mut a = ROOT;
        loop {
            a = if bits.bit()? { self.tree[a].right } else { self.tree[a].left };
            if a >= self.tree.len() {
                return None;
            }
            if self.tree[a].code >= 0 {
                break;
            }
        }
        let code = self.tree[a].code as usize;
        self.update(a);
        Some(code)
    }
}

/// Historiken som förladdas innan avkodningen (så att filens början kan använda kopior).
const PRELOAD: usize = 2 * 32 * 96 + 4 * 256;

/// LZCOMP-avkomprimering av ett block. `version` är MTX-versionen (1 = utan körlängdsflagga).
pub fn lzcomp(data: &[u8], version: u8) -> Option<Vec<u8>> {
    let mut bits = Bits { data, pos: 0, buf: 0, left: 0 };
    let run_length = version != 1 && bits.bit()?;
    let mut dist = Ahuff::new(8);
    let mut len = Ahuff::new(8);
    let out_len = bits.value(24)? as usize;
    let mut ranges = 1usize;
    while 1 + (1usize << (3 * ranges)) - 1 < out_len {
        ranges += 1;
    }
    let dup2 = 256 + 8 * ranges;
    let mut sym = Ahuff::new(dup2 + 3);
    let mut buf: Vec<u8> = Vec::with_capacity(PRELOAD + out_len);
    for k in 0..32u8 {
        for j in 0..96u8 {
            buf.push(k);
            buf.push(j);
        }
    }
    for j in 0..=255u8 {
        buf.extend_from_slice(&[j; 4]);
    }
    while buf.len() - PRELOAD < out_len {
        let s = sym.read(&mut bits)?;
        let pos = buf.len();
        if s < 256 {
            buf.push(s as u8);
        } else if s >= dup2 {
            let back = 2 * (s - dup2 + 1);
            buf.push(buf[pos.checked_sub(back)?]);
        } else {
            // Kopia: längden i 2-bitarsgrupper med stoppbit, sedan avståndet i 3-bitarsgrupper.
            let first = s - 256;
            let n_dist = first / 8 + 1;
            let mut b = first % 8;
            let mut length = 0usize;
            loop {
                length = length << 2 | (b & 3);
                if b & 4 == 0 {
                    break;
                }
                b = len.read(&mut bits)?;
            }
            length += 2;
            let mut distance = 0usize;
            for _ in 0..n_dist {
                distance = distance << 3 | dist.read(&mut bits)?;
            }
            distance += 1;
            if distance >= 512 {
                length += 1;
            }
            let start = (pos + 1).checked_sub(distance + length)?;
            for j in 0..length {
                let v = buf[start + j];
                buf.push(v);
            }
        }
    }
    buf.truncate(PRELOAD + out_len);
    let out = buf.split_off(PRELOAD);
    Some(if run_length { unrun(&out) } else { out })
}

/// Körlängdsavkodning: första byten är flyktbyten; flykt + antal + värde ger en följd, flykt + 0 en flyktbyte.
fn unrun(data: &[u8]) -> Vec<u8> {
    let Some((&esc, rest)) = data.split_first() else { return Vec::new() };
    let mut out = Vec::with_capacity(rest.len());
    let mut i = 0;
    while i < rest.len() {
        let v = rest[i];
        i += 1;
        if v != esc {
            out.push(v);
            continue;
        }
        let Some(&count) = rest.get(i) else { break };
        i += 1;
        if count == 0 {
            out.push(esc);
        } else if let Some(&val) = rest.get(i) {
            i += 1;
            out.extend(std::iter::repeat_n(val, count as usize));
        }
    }
    out
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        Some((self.u8()? as u16) << 8 | self.u8()? as u16)
    }
    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
    /// 255USHORT.
    fn u255(&mut self) -> Option<u16> {
        Some(match self.u8()? {
            253 => self.u16()?,
            254 => 506 + self.u8()? as u16,
            255 => 253 + self.u8()? as u16,
            c => c as u16,
        })
    }
}

/// En punkts förskjutning ur trippelkodningen (flaggans index 0..127 och de följande byten).
fn triplet(r: &mut Reader<'_>, flag: u8) -> Option<(i32, i32)> {
    let f = flag as i32;
    let sign = |bit: i32, v: i32| if bit & 1 == 1 { v } else { -v };
    Some(match flag {
        0..=9 => (0, sign(f, ((f & 14) << 7) + r.u8()? as i32)),
        10..=19 => (sign(f, (((f - 10) & 14) << 7) + r.u8()? as i32), 0),
        20..=83 => {
            let b0 = f - 20;
            let b1 = r.u8()? as i32;
            (sign(f, 1 + (b0 & 0x30) + (b1 >> 4)), sign(f >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)))
        }
        84..=119 => {
            let b0 = f - 84;
            let (x, y) = (r.u8()? as i32, r.u8()? as i32);
            (sign(f, 1 + ((b0 / 12) << 8) + x), sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + y))
        }
        120..=123 => {
            let (a, b, c) = (r.u8()? as i32, r.u8()? as i32, r.u8()? as i32);
            (sign(f, (a << 4) + (b >> 4)), sign(f >> 1, ((b & 0x0f) << 8) + c))
        }
        _ => {
            let (a, b, c, d) = (r.u8()? as i32, r.u8()? as i32, r.u8()? as i32, r.u8()? as i32);
            (sign(f, (a << 8) + b), sign(f >> 1, (c << 8) + d))
        }
    })
}

fn put16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_be_bytes());
}

/// En CTF-glyf → TrueType-glyf (utan instruktioner). Tom vektor för en tom glyf.
fn glyph(r: &mut Reader<'_>) -> Option<Vec<u8>> {
    let mut n = r.i16()?;
    let mut out = Vec::new();
    if n < 0 {
        // Sammansatt: begränsningsrutan och komponenterna som i TrueType.
        put16(&mut out, 0xFFFF);
        out.extend_from_slice(r.bytes(8)?);
        let mut instr = false;
        loop {
            let flags = r.u16()?;
            put16(&mut out, flags);
            out.extend_from_slice(r.bytes(2)?);
            let mut extra = if flags & 0x0001 != 0 { 4 } else { 2 };
            extra += if flags & 0x0008 != 0 {
                2
            } else if flags & 0x0040 != 0 {
                4
            } else if flags & 0x0080 != 0 {
                8
            } else {
                0
            };
            out.extend_from_slice(r.bytes(extra)?);
            instr |= flags & 0x0100 != 0;
            if flags & 0x0020 == 0 {
                break;
            }
        }
        if instr {
            r.u255()?;
            r.u255()?;
            put16(&mut out, 0);
        }
        return Some(out);
    }
    if n == 0 {
        // Tom glyf: bara antalet konturer.
        return Some(out);
    }
    let mut bbox = None;
    if n == 0x7FFF {
        n = r.i16()?;
        bbox = Some([r.i16()?, r.i16()?, r.i16()?, r.i16()?]);
        // Ett negativt antal blir `as usize` ≈ 2^64 och får `with_capacity` att panika.
        if n < 0 {
            return None;
        }
    }
    let n = n as usize;
    let mut ends = Vec::with_capacity(n);
    let mut total = 0usize;
    for i in 0..n {
        let v = r.u255()? as usize;
        total = if i == 0 { v + 1 } else { total + v };
        // Slutindexen skrivs som u16; fler punkter får inte trunkeras tyst till en trasig glyf.
        ends.push(u16::try_from(total.checked_sub(1)?).ok()?);
    }
    let flags = r.bytes(total)?.to_vec();
    let (mut x, mut y) = (0i32, 0i32);
    let mut pts = Vec::with_capacity(total);
    for &f in &flags {
        let (dx, dy) = triplet(r, f & 0x7F)?;
        x += dx;
        y += dy;
        pts.push((x, y, f & 0x80 == 0));
    }
    r.u255()?;
    r.u255()?;
    if n == 0 {
        return Some(out);
    }
    let bb = bbox.unwrap_or_else(|| {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &(px, py, _) in &pts {
            x0 = x0.min(px);
            y0 = y0.min(py);
            x1 = x1.max(px);
            y1 = y1.max(py);
        }
        if pts.is_empty() { [0; 4] } else { [x0 as i16, y0 as i16, x1 as i16, y1 as i16] }
    });
    put16(&mut out, n as u16);
    for v in bb {
        put16(&mut out, v as u16);
    }
    for e in &ends {
        put16(&mut out, *e);
    }
    put16(&mut out, 0);
    // Flaggor: bara på-kurvan-biten; koordinaterna som 16-bitars förskjutningar.
    out.extend(pts.iter().map(|p| p.2 as u8));
    let mut prev = 0;
    for p in &pts {
        put16(&mut out, (p.0 - prev) as i16 as u16);
        prev = p.0;
    }
    prev = 0;
    for p in &pts {
        put16(&mut out, (p.1 - prev) as i16 as u16);
        prev = p.1;
    }
    Some(out)
}

/// Bygger ett sfnt av tabellerna (taggen, innehållet), sorterade efter tagg.
fn build_sfnt(version: u32, mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|t| t.0);
    let n = tables.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    put16(&mut out, n);
    let es = 15 - n.max(1).leading_zeros() as u16;
    put16(&mut out, (1 << es) * 16);
    put16(&mut out, es);
    put16(&mut out, n * 16 - (1 << es) * 16);
    let mut off = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        let mut sum = 0u32;
        for c in data.chunks(4) {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            sum = sum.wrapping_add(u32::from_be_bytes(w));
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        off = 12 + 16 * tables.len() + body.len();
    }
    out.extend(body);
    out
}

/// CTF (avkomprimerat block 1) → TrueType.
pub fn ctf_to_ttf(ctf: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader { d: ctf, p: 0 };
    let version = u32::from_be_bytes(r.bytes(4)?.try_into().ok()?);
    let n = r.u16()? as usize;
    r.bytes(6)?;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mut glyf: Option<&[u8]> = None;
    let mut num_glyphs = 0usize;
    for _ in 0..n {
        let tag: [u8; 4] = r.bytes(4)?.try_into().ok()?;
        r.bytes(4)?;
        let off = u32::from_be_bytes(r.bytes(4)?.try_into().ok()?) as usize;
        let len = u32::from_be_bytes(r.bytes(4)?.try_into().ok()?) as usize;
        match &tag {
            b"loca" | b"cvt " | b"fpgm" | b"prep" | b"hdmx" | b"VDMX" | b"LTSH" | b"DSIG" => continue,
            _ => {}
        }
        let data = ctf.get(off..off.checked_add(len)?)?;
        if &tag == b"glyf" {
            glyf = Some(data);
            continue;
        }
        if &tag == b"maxp" {
            num_glyphs = u16::from_be_bytes([*data.get(4)?, *data.get(5)?]) as usize;
        }
        tables.push((tag, data.to_vec()));
    }
    let Some(glyf) = glyf else {
        // CFF-typsnitt (`OTTO`): ingen glyf att återskapa.
        return (version == 0x4F54_544F).then(|| build_sfnt(version, tables));
    };
    let mut gr = Reader { d: glyf, p: 0 };
    let mut new_glyf = Vec::new();
    let mut loca = Vec::with_capacity(4 * (num_glyphs + 1));
    for _ in 0..num_glyphs {
        loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        let g = glyph(&mut gr)?;
        new_glyf.extend_from_slice(&g);
        while new_glyf.len() % 4 != 0 {
            new_glyf.push(0);
        }
    }
    loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
    // head: långa loca-poster, ingen kontrollsumma.
    for (tag, data) in &mut tables {
        if tag == b"head" && data.len() >= 54 {
            data[8..12].copy_from_slice(&[0; 4]);
            data[50..52].copy_from_slice(&1u16.to_be_bytes());
        }
        if tag == b"maxp" && data.len() >= 32 {
            // Inga instruktioner kvar.
            data[26..28].copy_from_slice(&0u16.to_be_bytes());
        }
    }
    tables.push((*b"glyf", new_glyf));
    tables.push((*b"loca", loca));
    Some(build_sfnt(version, tables))
}

/// Hela MTX-filen (EOT:ens typsnittsdata) → TrueType.
pub fn decode(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 10 {
        return None;
    }
    let u24 = |o: usize| (data[o] as usize) << 16 | (data[o + 1] as usize) << 8 | data[o + 2] as usize;
    let version = data[0];
    let off2 = u24(4);
    if off2 < 10 || off2 > data.len() {
        return None;
    }
    let ctf = lzcomp(&data[10..off2], version)?;
    ctf_to_ttf(&ctf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kodare (motsvarigheten till specifikationens skrivväg) för provet: bara bokstavliga symboler.
    struct BitWriter {
        out: Vec<u8>,
        cur: u8,
        n: u32,
    }

    impl BitWriter {
        fn bit(&mut self, b: bool) {
            self.cur = self.cur << 1 | b as u8;
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.cur);
                self.cur = 0;
                self.n = 0;
            }
        }
        fn finish(mut self) -> Vec<u8> {
            while self.n != 0 {
                self.bit(false);
            }
            self.out
        }
    }

    impl Ahuff {
        /// Skriver symbolens kod (vägen från roten) och uppdaterar trädet som läsaren gör.
        fn write(&mut self, w: &mut BitWriter, sym: usize) {
            let mut path = Vec::new();
            let mut a = self.index[sym];
            while a != ROOT {
                let up = self.tree[a].up;
                path.push(self.tree[up].right == a);
                a = up;
            }
            for b in path.into_iter().rev() {
                w.bit(b);
            }
            self.update(self.index[sym]);
        }
    }

    fn compress_literals(data: &[u8], dup2_at: &[usize]) -> Vec<u8> {
        let mut w = BitWriter { out: Vec::new(), cur: 0, n: 0 };
        w.bit(false); // ingen körlängd
        for i in (0..24).rev() {
            w.bit(data.len() >> i & 1 == 1);
        }
        let mut ranges = 1usize;
        while (1usize << (3 * ranges)) < data.len() {
            ranges += 1;
        }
        let dup2 = 256 + 8 * ranges;
        let mut sym = Ahuff::new(dup2 + 3);
        for (i, &b) in data.iter().enumerate() {
            if dup2_at.contains(&i) {
                sym.write(&mut w, dup2);
            } else {
                sym.write(&mut w, b as usize);
            }
        }
        w.finish()
    }

    #[test]
    fn lzcomp_bokstavligt_och_dup2() {
        let data = b"abcabcXYXYZ hej hej".to_vec();
        // Position 5 ('c') är samma som två steg bakåt? Nej: 'a','b','c','a','b' → index 4 = 'b' = data[2]? Använd DUP2 där det stämmer.
        let dups: Vec<usize> = (2..data.len()).filter(|&i| data[i] == data[i - 2]).collect();
        assert!(!dups.is_empty());
        let packed = compress_literals(&data, &dups);
        assert_eq!(lzcomp(&packed, 3).as_deref(), Some(&data[..]));
        // Version 1 saknar körlängdsbiten.
        let mut v1 = BitWriter { out: Vec::new(), cur: 0, n: 0 };
        for i in (0..24).rev() {
            v1.bit(3 >> i & 1 == 1);
        }
        let mut sym = Ahuff::new(256 + 8 + 3);
        for b in *b"xyz" {
            sym.write(&mut v1, b as usize);
        }
        assert_eq!(lzcomp(&v1.finish(), 1).as_deref(), Some(&b"xyz"[..]));
        // Trasig data ger None, ingen panik.
        assert!(lzcomp(&[0xFF, 0xFF], 3).is_none());
    }

    /// Skriver en kopia som specifikationens kodare. Avståndet räknas från kopians sista byte: källan
    /// börjar `distance + length − 1` byte bakåt.
    fn write_copy(w: &mut BitWriter, sym: &mut Ahuff, len: &mut Ahuff, dist: &mut Ahuff, length: usize, distance: usize) {
        let value = length - if distance >= 512 { 3 } else { 2 };
        let groups = |v: usize, width: u32| {
            let mut g = Vec::new();
            let mut v = v;
            loop {
                g.push(v & ((1 << width) - 1));
                v >>= width;
                if v == 0 {
                    break;
                }
            }
            g.reverse();
            g
        };
        let lg = groups(value, 2);
        let dg = groups(distance - 1, 3);
        let more = |i: usize| if i + 1 < lg.len() { 4 } else { 0 };
        sym.write(w, 256 + (dg.len() - 1) * 8 + (lg[0] | more(0)));
        for i in 1..lg.len() {
            len.write(w, lg[i] | more(i));
        }
        for g in dg {
            dist.write(w, g);
        }
    }

    #[test]
    fn lzcomp_kopior() {
        // 700 byte bokstäver, sedan 6 byte från 8 bakåt (avstånd 3) och 10 byte från 620 bakåt (avstånd
        // 611, över 512: längden räknas då från 3).
        let mut data: Vec<u8> = (0..700u32).map(|i| b'a' + (i * 7 % 26) as u8).collect();
        let lits = data.len();
        let from = data.len() - 8;
        data.extend_from_within(from..from + 6);
        let from = data.len() - 620;
        data.extend_from_within(from..from + 10);
        let mut w = BitWriter { out: Vec::new(), cur: 0, n: 0 };
        w.bit(false);
        for i in (0..24).rev() {
            w.bit(data.len() >> i & 1 == 1);
        }
        let mut ranges = 1usize;
        while (1usize << (3 * ranges)) < data.len() {
            ranges += 1;
        }
        let (mut dist, mut len) = (Ahuff::new(8), Ahuff::new(8));
        let mut sym = Ahuff::new(256 + 8 * ranges + 3);
        for &b in &data[..lits] {
            sym.write(&mut w, b as usize);
        }
        write_copy(&mut w, &mut sym, &mut len, &mut dist, 6, 3);
        write_copy(&mut w, &mut sym, &mut len, &mut dist, 10, 611);
        assert_eq!(lzcomp(&w.finish(), 3).as_deref(), Some(&data[..]));
    }

    #[test]
    fn glyf_med_negativt_konturantal_ger_none() {
        // 0x7FFF följt av ett negativt antal fick `n as usize` att bli enormt (with_capacity-panik).
        let mut g = Vec::new();
        g.extend_from_slice(&0x7FFFi16.to_be_bytes());
        g.extend_from_slice(&(-1i16).to_be_bytes());
        g.extend_from_slice(&[0; 8]); // begränsningsrutan
        assert!(glyph(&mut Reader { d: &g, p: 0 }).is_none());
    }

    #[test]
    fn korlangd() {
        assert_eq!(unrun(&[0xAA, 1, 0xAA, 3, 7, 2, 0xAA, 0]), vec![1, 7, 7, 7, 2, 0xAA]);
    }

    #[test]
    fn ctf_glyf_till_truetype() {
        // En kvadrat (fyra punkter i en kontur) och en sammansatt glyf som pekar på den.
        let mut g = Vec::new();
        g.extend_from_slice(&1i16.to_be_bytes()); // en kontur
        g.push(3); // sista punktens index i första konturen
        // Flaggor: (0,+100) index 1 (y positiv, 8 bitar), (+100,0) index 11, (0,-100) index 0, på kurvan.
        g.extend_from_slice(&[1, 11, 0, 11 - 1]);
        g.extend_from_slice(&[100, 100, 100, 100]);
        g.extend_from_slice(&[0, 0]); // pushCount, codeSize
        g.extend_from_slice(&(-1i16).to_be_bytes());
        g.extend_from_slice(&[0, 0, 0, 0, 0, 100, 0, 100]);
        g.extend_from_slice(&0x0002u16.to_be_bytes()); // ARGS_ARE_XY_VALUES, en komponent
        g.extend_from_slice(&0u16.to_be_bytes());
        g.extend_from_slice(&[5, 5]);
        g.extend_from_slice(&0i16.to_be_bytes()); // tom glyf: bara antalet konturer
        let mut r = Reader { d: &g, p: 0 };
        let a = glyph(&mut r).unwrap();
        let b = glyph(&mut r).unwrap();
        let c = glyph(&mut r).unwrap();
        assert_eq!(r.p, g.len(), "hela glyftabellen läst");
        assert!(c.is_empty());
        assert_eq!(&b[..2], &[0xFF, 0xFF]);
        // numberOfContours 1, bbox (0,0)-(100,100), slutpunkt 3, inga instruktioner.
        assert_eq!(&a[..14], &[0, 1, 0, 0, 0, 0, 0, 100, 0, 100, 0, 3, 0, 0]);
        // Bygg ett typsnitt av glyferna och läs det med ttf-parser.
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&3u16.to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
        let hmtx = vec![0u8; 4 + 2 * 2];
        let mut glyf_ctf = g.clone();
        while glyf_ctf.len() % 4 != 0 {
            glyf_ctf.push(0);
        }
        let ctf = build_sfnt(0x0001_0000, vec![(*b"head", head), (*b"maxp", maxp), (*b"hhea", hhea), (*b"hmtx", hmtx), (*b"glyf", g)]);
        let ttf = ctf_to_ttf(&ctf).unwrap();
        let face = ttf_parser::Face::parse(&ttf, 0).unwrap();
        assert_eq!(face.number_of_glyphs(), 3);
        let bb = face.glyph_bounding_box(ttf_parser::GlyphId(0)).unwrap();
        assert_eq!((bb.x_min, bb.y_min, bb.x_max, bb.y_max), (0, 0, 100, 100));
        let bb = face.glyph_bounding_box(ttf_parser::GlyphId(1)).unwrap();
        assert_eq!((bb.x_min, bb.y_min), (5, 5));
    }
}

/// Avkodar alla inbäddade typsnitt i presentationerna under `KLADD_PPTX_REFS` och jämför teckenbredderna
/// med Carlito där typsnittet är Calibri (metriskt lika). Körs för hand (referensfilerna ligger inte i git).
#[test]
#[ignore]
fn mtx_referenspresentationer() {
    let Some(refs) = std::env::var_os("KLADD_PPTX_REFS") else { return };
    let mut ok = 0;
    let mut fail = Vec::new();
    for e in std::fs::read_dir(refs).unwrap().flatten() {
        let Ok(bytes) = std::fs::read(e.path().join("deck.pptx")) else { continue };
        let Ok(pkg) = super::package::Package::open(&bytes) else { continue };
        for part in pkg.parts.keys().filter(|p| p.ends_with(".fntdata")) {
            let data = pkg.read(part).unwrap();
            match super::fonts::decode_embedded(data, part) {
                Some(ttf) => {
                    let face = ttf_parser::Face::parse(&ttf, 0).unwrap();
                    let fam = face.names().into_iter().find(|n| n.name_id == ttf_parser::name_id::FAMILY).and_then(|n| n.to_string()).unwrap_or_default();
                    let a = face.glyph_index('a').and_then(|g| face.outline_glyph(g, &mut NullBuilder)).is_some();
                    println!("{} {part}: {fam}, {} glyfer, a-kontur {a}", e.file_name().to_string_lossy(), face.number_of_glyphs());
                    if fam == "Calibri" {
                        let data = crate::word::fontmap::Bundled::Carlito.data(face.is_bold(), face.is_italic());
                        let carlito = ttf_parser::Face::parse(data, 0).unwrap();
                        for c in "Hamburgefonstiv".chars() {
                            let w = |f: &ttf_parser::Face| f.glyph_index(c).and_then(|g| f.glyph_hor_advance(g)).map_or(0.0, |a| a as f32 / f.units_per_em() as f32);
                            assert!((w(&face) - w(&carlito)).abs() < 0.01, "{c}: {} mot {}", w(&face), w(&carlito));
                        }
                    }
                    ok += 1;
                }
                None => fail.push(format!("{} {part}", e.file_name().to_string_lossy())),
            }
        }
    }
    // PDF med de avkodade typsnitten (krilla bäddar in och delmängdar dem).
    if let Some(out) = std::env::var_os("KLADD_MTX_PDF") {
        for name in ["cncf-overview-ja", "cppcon19-macrofree", "psconf19-pscore"] {
            let Ok(bytes) = std::fs::read(std::path::Path::new(&std::env::var_os("KLADD_PPTX_REFS").unwrap()).join(name).join("deck.pptx")) else { continue };
            let deck = super::Deck::open(&bytes).unwrap();
            let pdf = super::pdf::write(&deck, &(0..deck.slides.len().min(6)).collect::<Vec<_>>()).unwrap();
            std::fs::write(std::path::Path::new(&out).join(format!("{name}.pdf")), pdf).unwrap();
        }
    }
    println!("{ok} avkodade, misslyckade: {fail:?}");
    assert!(fail.is_empty());
}

#[cfg(test)]
struct NullBuilder;

#[cfg(test)]
impl ttf_parser::OutlineBuilder for NullBuilder {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

