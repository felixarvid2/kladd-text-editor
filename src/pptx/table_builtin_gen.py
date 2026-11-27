#!/usr/bin/env python3
"""Genererar src/pptx/table_builtin.xml och table_builtin_sources.md: PowerPoints 74 inbyggda tabellformat.

PowerPoint skriver bara de format som används till ppt/tableStyles.xml, och vissa program (python-pptx,
äldre versioner av PowerPoint) skriver inte ens dem. Formaten här är tagna ur tableStyles.xml i filer
som PowerPoint har sparat och som ligger i licensierade provsamlingar:

  - Apache POI test-data (Apache-2.0)
  - Open-XML-SDK test assets (MIT)
  - ShapeCrawler test assets (MIT)
  - python-pptx features/steps/test_files (MIT)
  - docx4j (Apache-2.0)

Varje GUID tas ur en fil där den finns. Saknas den härleds den ur ett syskon i samma familj genom att
byta accentfärgen (`accentK` → `accentN`); regeln provas på alla familjer där flera medlemmar finns, och
skriptet stannar om ett observerat format inte stämmer med regeln. Dark Style 1 med accent finns inte i
någon licensierad fil: där används regeln ur oxml-drawing (tensorbee/rdocx, MIT OR Apache-2.0) och
formaten märks som ej verifierade. GUID → familj och accent följer Microsofts publicerade lista
(samma tabell som oxml-drawing har).

Användning: table_builtin_gen.py <mapp med provsamlingarna> [<mapp med andra filer att kontrollera mot>]
Den andra mappen används bara för att jämföra härledda format med riktiga; inget kopieras därifrån.
"""

import collections
import glob
import os
import re
import sys
import zipfile

# Familjer i Microsofts ordning: (familj, [(guid, variant)]). Variant 0 = utan accent.
FAMILIES = [
    ("Themed Style 1", [("{2D5ABB26-0587-4C30-8999-92F81FD0307C}", "nogrid"), ("{3C2FFA5D-87B4-456A-9821-1D502468CF0F}", 1),
                        ("{284E427A-3D55-4303-BF80-6455036E1DE7}", 2), ("{69C7853C-536D-4A76-A0AE-DD22124D55A5}", 3),
                        ("{775DCB02-9BB8-47FD-8907-85C794F793BA}", 4), ("{35758FB7-9AC5-4552-8A53-C91805E547FA}", 5),
                        ("{08FB837D-C827-4EFA-A057-4D05807E0F7C}", 6)]),
    ("Themed Style 2", [("{5940675A-B579-460E-94D1-54222C63F5DA}", "grid"), ("{D113A9D2-9D6B-4929-AA2D-F23B5EE8CBE7}", 1),
                        ("{18603FDC-E32A-4AB5-989C-0864C3EAD2B8}", 2), ("{306799F8-075E-4A3A-A7F6-7FBC6576F1A4}", 3),
                        ("{E269D01E-BC32-4049-B463-5C60D7B0CCD2}", 4), ("{327F97BB-C833-4FB7-BDE5-3F7075034690}", 5),
                        ("{638B1855-1B75-4FBE-930C-398BA8C253C6}", 6)]),
    ("Light Style 1", [("{9D7B26C5-4107-4FEC-AEDC-1716B250A1EF}", 0), ("{3B4B98B0-60AC-42C2-AFA5-B58CD77FA1E5}", 1),
                       ("{0E3FDE45-AF77-4B5C-9715-49D594BDF05E}", 2), ("{C083E6E3-FA7D-4D7B-A595-EF9225AFEA82}", 3),
                       ("{D27102A9-8310-4765-A935-A1911B00CA55}", 4), ("{5FD0F851-EC5A-4D38-B0AD-8093EC10F338}", 5),
                       ("{68D230F3-CF80-4859-8CE7-A43EE81993B5}", 6)]),
    ("Light Style 2", [("{7E9639D4-E3E2-4D34-9284-5A2195B3D0D7}", 0), ("{69012ECD-51FC-41F1-AA8D-1B2483CD663E}", 1),
                       ("{72833802-FEF1-4C79-8D5D-14CF1EAF98D9}", 2), ("{F2DE63D5-997A-4646-A377-4702673A728D}", 3),
                       ("{17292A2E-F333-43FB-9621-5CBBE7FDCDCB}", 4), ("{5A111915-BE36-4E01-A7E5-04B1672EAD32}", 5),
                       ("{912C8C85-51F0-491E-9774-3900AFEF0FD7}", 6)]),
    ("Light Style 3", [("{616DA210-FB5B-4158-B5E0-FEB733F419BA}", 0), ("{BC89EF96-8CEA-46FF-86C4-4CE0E7609802}", 1),
                       ("{5DA37D80-6434-44D0-A028-1B22A696006F}", 2), ("{8799B23B-EC83-4686-B30A-512413B5E67A}", 3),
                       ("{ED083AE6-46FA-4A59-8FB0-9F97EB10719F}", 4), ("{BDBED569-4797-4DF1-A0F4-6AAB3CD982D8}", 5),
                       ("{E8B1032C-EA38-4F05-BA0D-38AFFFC7BED3}", 6)]),
    ("Medium Style 1", [("{793D81CF-94F2-401A-BA57-92F5A7B2D0C5}", 0), ("{B301B821-A1FF-4177-AEE7-76D212191A09}", 1),
                        ("{9DCAF9ED-07DC-4A11-8D7F-57B35C25682E}", 2), ("{1FECB4D8-DB02-4DC6-A0A2-4F2EBAE1DC90}", 3),
                        ("{1E171933-4619-4E11-9A3F-F7608DF75F80}", 4), ("{FABFCF23-3B69-468F-B69F-88F6DE6A72F2}", 5),
                        ("{10A1B5D5-9B99-4C35-A422-299274C87663}", 6)]),
    ("Medium Style 2", [("{073A0DAA-6AF3-43AB-8588-CEC1D06C72B9}", 0), ("{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}", 1),
                        ("{21E4AEA4-8DFA-4A89-87EB-49C32662AFE0}", 2), ("{F5AB1C69-6EDB-4FF4-983F-18BD219EF322}", 3),
                        ("{00A15C55-8517-42AA-B614-E9B94910E393}", 4), ("{7DF18680-E054-41AD-8BC1-D1AEF772440D}", 5),
                        ("{93296810-A885-4BE3-A3E7-6D5BEEA58F35}", 6)]),
    ("Medium Style 3", [("{8EC20E35-A176-4012-BC5E-935CFFF8708E}", 0), ("{6E25E649-3F16-4E02-A733-19D2CDBF48F0}", 1),
                        ("{85BE263C-DBD7-4A20-BB59-AAB30ACAA65A}", 2), ("{EB344D84-9AFB-497E-A393-DC336BA19D2E}", 3),
                        ("{EB9631B5-78F2-41C9-869B-9F39066F8104}", 4), ("{74C1A8A3-306A-4EB7-A6B1-4F7E0EB9C5D6}", 5),
                        ("{2A488322-F2BA-4B5B-9748-0D474271808F}", 6)]),
    ("Medium Style 4", [("{D7AC3CCA-C797-4891-BE02-D94E43425B78}", 0), ("{69CF1AB2-1976-4502-BF36-3FF5EA218861}", 1),
                        ("{8A107856-5554-42FB-B03E-39F5DBC370BA}", 2), ("{0505E3EF-67EA-436B-97B2-0124C06EBD24}", 3),
                        ("{C4B1156A-380E-4F78-BDF5-A606A8083BF9}", 4), ("{22838BEF-8BB2-4498-84A7-C5851F593DF1}", 5),
                        ("{16D9F66E-5EB9-4882-86FB-DCBF35E3C3E4}", 6)]),
    ("Dark Style 1", [("{E8034E78-7F5D-4C2E-B375-FC64B27BC917}", 0), ("{125E5076-3810-47DD-B79F-674D7AD40C01}", 1),
                      ("{37CE84F3-28C3-443E-9E96-99CF82512B78}", 2), ("{D03447BB-5D67-496B-8E87-E561075AD55C}", 3),
                      ("{E929F9F4-4A8F-4326-A1B4-22849713DDAB}", 4), ("{8FD4443E-F989-4FC4-A0C8-D5A2AF1F390B}", 5),
                      ("{AF606853-7671-496A-8E4F-DF71F8EC918B}", 6)]),
    ("Dark Style 2", [("{5202B0CA-FC54-4496-8BCA-5EF66A818D29}", 0), ("{0660B408-B3CF-4A94-85FC-2B1E0A45F4A2}", 1),
                      ("{91EBBBCC-DAD2-459C-BE2E-F6DE35CF9A28}", 3), ("{46F890A9-2807-4EBB-B81D-B2AA78EC7F39}", 5)]),
]

LICENSES = {"poi": "Apache POI, Apache-2.0", "openxmlsdk": "Open-XML-SDK, MIT", "shapecrawler": "ShapeCrawler, MIT",
            "python-pptx": "python-pptx, MIT", "docx4j": "docx4j, Apache-2.0"}


def style_name(family, variant):
    if variant == "nogrid":
        return "No Style, No Grid"
    if variant == "grid":
        return "No Style, Table Grid"
    if variant == 0:
        return family
    if family == "Dark Style 2":
        return "%s - Accent %d/Accent %d" % (family, variant, variant + 1)
    return "%s - Accent %d" % (family, variant)


def norm(x):
    """Tar bort skillnader utan betydelse: namnet, platshållarfärgen i fontRef och tomma formatdelar."""
    x = re.sub(r'\s+styleName="[^"]*"', "", x)
    x = re.sub(r'<a:tblStyle\s+', "<a:tblStyle ", x)
    x = x.replace('<a:prstClr val="black"/>', '<a:scrgbClr r="0" g="0" b="0"/>')
    x = re.sub(r'<a:fontRef idx="(\w+)"/>', r'<a:fontRef idx="\1"><a:scrgbClr r="0" g="0" b="0"/></a:fontRef>', x)
    x = re.sub(r"<a:(\w+)><a:tcStyle><a:tcBdr/></a:tcStyle></a:\1>", "", x)
    x = re.sub(r">\s+<", "><", x)
    return x.strip()


def collect(root):
    found = collections.defaultdict(list)
    files = sorted(glob.glob(root + "/**/*.pptx", recursive=True) + glob.glob(root + "/**/*.potx", recursive=True))
    for p in files:
        try:
            x = zipfile.ZipFile(p).read("ppt/tableStyles.xml").decode("utf-8", "replace")
        except Exception:
            continue
        for m in re.finditer(r'<a:tblStyle\b[^>]*styleId="(\{[^}]*\})"[^>]*>.*?</a:tblStyle>', x, re.S):
            found[m.group(1).upper()].append((os.path.relpath(p, root), norm(m.group(0))))
    return found


def recolor(x, src, dst):
    """Byter accentfärgen `src` (int eller par) mot `dst` i ett format."""
    if isinstance(src, tuple):
        for a, b in zip(src, dst):
            x = x.replace('val="accent%d"' % a, 'val="@%d"' % b)
        return re.sub(r'val="@(\d)"', r'val="accent\1"', x)
    if src == 0:
        raise ValueError
    return x.replace('val="accent%d"' % src, 'val="accent%d"' % dst)


def with_id(x, guid):
    return re.sub(r'styleId="\{[^}]*\}"', 'styleId="%s"' % guid, x, count=1)


def dark1_accent(plain, n):
    """Dark Style 1 med accent ur formatet utan accent (regeln i oxml-drawing; ej verifierad mot PowerPoint)."""
    a = '<a:schemeClr val="accent%d"' % n

    def part(x, name, f):
        return re.sub(r"(<a:%s>.*?</a:%s>)" % (name, name), lambda m: f(m.group(1)), x, count=1, flags=re.S)

    tint = lambda v: '<a:schemeClr val="dk1"><a:tint val="%d"/></a:schemeClr>' % v
    x = part(plain, "wholeTbl", lambda s: s.replace(tint(20000), a + "/>"))
    for name in ("band1H", "band1V", "lastCol", "firstCol"):
        x = part(x, name, lambda s: s.replace(tint(40000), a + '><a:shade val="60000"/></a:schemeClr>').replace(tint(60000), a + '><a:shade val="60000"/></a:schemeClr>'))
    x = part(x, "lastRow", lambda s: s.replace(tint(60000), a + '><a:shade val="40000"/></a:schemeClr>'))
    return x


def main():
    lic_root = sys.argv[1]
    check_root = sys.argv[2] if len(sys.argv) > 2 else None
    found = collect(lic_root)
    check = collect(check_root) if check_root else {}
    out, sources, problems = [], [], []
    for family, members in FAMILIES:
        accents = [(g, v) for g, v in members if isinstance(v, int) and v > 0]
        # Observerade medlemmar: välj den variant som stämmer med syskonen (annars den vanligaste).
        chosen = {}
        for g, v in members:
            vs = collections.Counter(x for _, x in found.get(g, []))
            if not vs:
                continue
            cands = [x for x, _ in vs.most_common()]
            if isinstance(v, int) and v > 0 and len(cands) > 1:
                sib = [(g2, v2) for g2, v2 in accents if g2 != g and found.get(g2)]
                agree = [c for c in cands if any(with_id(recolor(c, v, v2 if family != "Dark Style 2" else (v2, v2 + 1)) if family != "Dark Style 2" else recolor(c, (v, v + 1), (v2, v2 + 1)), g2) in {x for _, x in found[g2]} for g2, v2 in sib)]
                if agree:
                    cands = agree + [c for c in cands if c not in agree]
            chosen[g] = cands[0]
        # Regeln (byt accentfärg) provas på alla observerade par i familjen.
        obs = [(g, v) for g, v in accents if g in chosen]
        for g1, v1 in obs:
            for g2, v2 in obs:
                if g1 == g2:
                    continue
                pair = family == "Dark Style 2"
                y = with_id(recolor(chosen[g1], (v1, v1 + 1) if pair else v1, (v2, v2 + 1) if pair else v2), g2)
                if y != chosen[g2]:
                    problems.append("%s: %s → %s stämmer inte" % (family, style_name(family, v1), style_name(family, v2)))
        for g, v in members:
            name = style_name(family, v)
            if g in chosen:
                x = chosen[g]
                srcs = [p for p, y in found[g] if y == x]
                src = srcs[0]
                note = "%s (%s)" % (src, LICENSES[src.split("/")[0]])
            elif isinstance(v, int) and v > 0 and obs:
                g1, v1 = obs[0]
                pair = family == "Dark Style 2"
                x = with_id(recolor(chosen[g1], (v1, v1 + 1) if pair else v1, (v, v + 1) if pair else v), g)
                note = "härlett ur %s (byte av accentfärg)" % style_name(family, v1)
                if pair or len(obs) < 2:
                    note += "; regeln inte provad i familjen"
            elif family == "Dark Style 1" and isinstance(v, int) and v > 0:
                x = with_id(dark1_accent(chosen[members[0][0]], v), g)
                note = "ej verifierat: härlett ur Dark Style 1 med regeln i oxml-drawing (MIT OR Apache-2.0)"
            elif v == 0 and obs:
                # Utan accent: accentfärgen byts mot dk1 (provat på Medium Style 1, 2 och 4 nedan).
                g1, v1 = obs[0]
                x = with_id(chosen[g1].replace('val="accent%d"' % v1, 'val="dk1"'), g)
                note = "härlett ur %s (accentfärgen → dk1)" % style_name(family, v1)
            else:
                problems.append("%s saknas" % name)
                continue
            if g in check and x not in {y for _, y in check[g]}:
                note += "; AVVIKER från en annan PowerPoint-fil"
            elif g in check and g not in chosen:
                note += "; stämmer med en PowerPoint-fil utanför provsamlingarna"
            x = x.replace("<a:tblStyle ", '<a:tblStyle styleName="%s" ' % name, 1)
            out.append(x)
            sources.append("| `%s` | %s | %s |" % (g, name, note))
    # Regeln för formaten utan accent provas där de finns.
    for family, members in FAMILIES:
        g0, v0 = members[0]
        if v0 != 0:
            continue
        obs = [(g, v) for g, v in members[1:] if found.get(g)]
        if found.get(g0) and obs:
            g1, v1 = obs[0]
            y = with_id(collections.Counter(x for _, x in found[g1]).most_common(1)[0][0].replace('val="accent%d"' % v1, 'val="dk1"'), g0)
            ok = y in {x for _, x in found[g0]}
            print("utan accent ur accent (%s): %s" % (family, "stämmer" if ok else "stämmer inte"), file=sys.stderr)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        sys.exit(1)
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "table_builtin.xml"), "w", encoding="utf-8") as f:
        f.write('<?xml version="1.0" encoding="UTF-8"?>\n')
        f.write("<!-- Genererad av table_builtin_gen.py; källor i table_builtin_sources.md. -->\n")
        f.write('<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}">\n')
        for x in out:
            f.write(x + "\n")
        f.write("</a:tblStyleLst>\n")
    with open(os.path.join(here, "table_builtin_sources.md"), "w", encoding="utf-8") as f:
        f.write("# Inbyggda tabellformat: källor\n\n")
        f.write("Genererad av `table_builtin_gen.py` (läs skriptets beskrivning). Formaten är PowerPoints egna, så\n")
        f.write("som PowerPoint skriver dem i `ppt/tableStyles.xml`; filerna nedan ligger i provsamlingarna\n")
        f.write("Apache POI (`test-data`, Apache-2.0), Open-XML-SDK (MIT), ShapeCrawler (MIT) och python-pptx (MIT).\n")
        f.write("GUID-listan är Microsofts (samma som i oxml-drawing, MIT OR Apache-2.0).\n\n")
        f.write("| GUID | Format | Källa |\n|---|---|---|\n")
        f.write("\n".join(sources) + "\n")
    print("%d format" % len(out), file=sys.stderr)


if __name__ == "__main__":
    main()
