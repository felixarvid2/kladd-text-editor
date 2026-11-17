"""Skolrapport i Word 365-stil: titelsida utan sidhuvud (w:titlePg), sidhuvud med logga (bild) till vänster och
rapportnamn till höger (högertabb; på jämna sidor en tabell utan ramar), sidfot "Sida X av Y" (PAGE/NUMPAGES som
w:fldSimple på udda sidor och som w:fldChar-sekvens på jämna), udda/jämna sidhuvuden (w:evenAndOddHeaders),
fotnoter och slutnoter, innehållsförteckning (sdt + TOC-fält med PAGEREF) och en faktaruta i marginalen
(mc:AlternateContent med wps och VML-reserv). Allt är påhittat.
Användning: python3 mkskolrapport.py <utkatalog>  ->  gen-skolrapport.docx"""
import sys, os, io
from xml.sax.saxutils import escape
from wordgen import *

ids = Ids()
W, H = 11906, 16838
ML, MR, MT, MB = 1417, 2835, 1417, 1417
TEXT_W = W - ML - MR   # 7654
REPORT = 'Hållbar skola: avfall och energi på Ekbackeskolan'

# sidnummer i innehållsförteckningen (uppmätta i LibreOffice, se SOURCES.md)
PAGES = {1: 3, 2: 3, 3: 3, 4: 4, 5: 4, 6: 4, 7: 4}


def logo_png():
    from PIL import Image, ImageDraw, ImageFont
    im = Image.new('RGB', (240, 240), (255, 255, 255))
    d = ImageDraw.Draw(im)
    d.ellipse((6, 6, 234, 234), fill=(21, 96, 130))
    d.ellipse((30, 30, 210, 210), outline=(255, 255, 255), width=6)
    f = ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf', 110)
    d.text((120, 122), 'E', font=f, fill=(255, 255, 255), anchor='mm')
    d.polygon([(120, 36), (150, 78), (90, 78)], fill=(233, 113, 50))
    buf = io.BytesIO()
    im.save(buf, 'PNG', optimize=True)
    return buf.getvalue()


def body(*parts, jc=True, ppr=''):
    content = ''.join(p if p.startswith('<') else run(p, ids=ids) for p in parts)
    return para(ids, content, ppr=('<w:jc w:val="both"/>' if jc else '') + ppr)


def fn_ref(n):
    return '<w:r><w:rPr><w:rStyle w:val="Fotnotsreferens"/></w:rPr><w:footnoteReference w:id="%d"/></w:r>' % n


def en_ref(n):
    return '<w:r><w:rPr><w:rStyle w:val="Slutnotsreferens"/></w:rPr><w:endnoteReference w:id="%d"/></w:r>' % n


HEADINGS = ['Inledning', 'Bakgrund', 'Metod', 'Resultat', 'Diskussion', 'Slutsats', 'Källor']


def heading(i, first=False):
    bm = 200000000 + i
    ppr = '<w:pageBreakBefore/>' if first else ''
    return ('<w:p w14:paraId="%s" w14:textId="77777777" w:rsidR="00A1B2C3" w:rsidRDefault="00D4E5F6"><w:pPr><w:pStyle w:val="Rubrik1"/>%s</w:pPr>'
            '<w:bookmarkStart w:id="%d" w:name="_Toc%d"/>%s<w:bookmarkEnd w:id="%d"/></w:p>'
            % (ids.para_id(), ppr, i, bm, run(HEADINGS[i - 1], ids=ids), i))


# ---------------------------------------------------------------- titelsida
title_page = (
    para(ids, '', ppr='<w:spacing w:before="2400" w:after="0"/>')
    + para(ids, run('Hållbar skola', ids=ids), style='Rubrik', ppr='<w:spacing w:after="120"/>')
    + para(ids, run('Avfall och energi på Ekbackeskolan', ids=ids), style='Underrubrik')
    + para(ids, '', ppr='<w:spacing w:before="1800"/>')
    + para(ids, run('Elsa Karlsson och Noah Berg', '<w:b/><w:bCs/><w:sz w:val="28"/><w:szCs w:val="28"/>', ids), ppr='<w:spacing w:after="60"/>')
    + para(ids, run('Klass 9B, Ekbackeskolan', ids=ids), ppr='<w:spacing w:after="60"/>')
    + para(ids, run('Handledare: Karin Öberg, lärare i naturkunskap', ids=ids), ppr='<w:spacing w:after="60"/>')
    + para(ids, run('Mars 2026', ids=ids), ppr='<w:spacing w:after="0"/>')
    + para(ids, '<w:r><w:br w:type="page"/></w:r>')
)

# ---------------------------------------------------------------- innehållsförteckning (sdt som i Word)
def toc():
    def entry(i, last=False):
        bm = '_Toc%d' % (200000000 + i)
        link = ('<w:hyperlink w:anchor="%s" w:history="1"><w:r><w:rPr><w:rStyle w:val="Hyperlnk"/><w:noProof/></w:rPr><w:t>%s</w:t></w:r>'
                '<w:r><w:rPr><w:noProof/><w:webHidden/></w:rPr><w:tab/></w:r>'
                '<w:r><w:rPr><w:noProof/><w:webHidden/></w:rPr><w:fldChar w:fldCharType="begin"/></w:r>'
                '<w:r><w:rPr><w:noProof/><w:webHidden/></w:rPr><w:instrText xml:space="preserve"> PAGEREF %s \\h </w:instrText></w:r>'
                '<w:r><w:rPr><w:noProof/><w:webHidden/></w:rPr><w:fldChar w:fldCharType="separate"/></w:r>'
                '<w:r><w:rPr><w:noProof/><w:webHidden/></w:rPr><w:t>%d</w:t></w:r>'
                '<w:r><w:rPr><w:noProof/><w:webHidden/></w:rPr><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink>'
                % (bm, escape(HEADINGS[i - 1]), bm, PAGES[i]))
        start = ''
        if i == 1:
            start = ('<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \\o "1-3" \\h \\z \\u </w:instrText></w:r>'
                     '<w:r><w:fldChar w:fldCharType="separate"/></w:r>')
        end = '' if not last else ''
        return para(ids, start + link + end, style='Innehll1',
                    ppr='<w:tabs><w:tab w:val="right" w:leader="dot" w:pos="%d"/></w:tabs>' % TEXT_W,
                    rpr_mark='<w:noProof/>')
    entries = ''.join(entry(i) for i in range(1, len(HEADINGS) + 1))
    entries += para(ids, '<w:r><w:rPr><w:b/><w:bCs/><w:noProof/></w:rPr><w:fldChar w:fldCharType="end"/></w:r>')
    return ('<w:sdt><w:sdtPr><w:id w:val="-1342177280"/><w:docPartObj><w:docPartGallery w:val="Table of Contents"/>'
            '<w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtEndPr><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:eastAsiaTheme="minorHAnsi" '
            'w:hAnsiTheme="minorHAnsi" w:cstheme="minorBidi"/><w:b/><w:bCs/><w:noProof/><w:color w:val="auto"/><w:kern w:val="2"/>'
            '<w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:sdtEndPr><w:sdtContent>%s%s</w:sdtContent></w:sdt>'
            % (para(ids, run('Innehållsförteckning', ids=ids), style='Innehllsfrteckningsrubrik'), entries))


# ---------------------------------------------------------------- brödtext
BOX = (
    para(ids, run('Fakta om skolan', '<w:b/><w:bCs/><w:color w:val="0F4761"/>', ids), ppr='<w:spacing w:after="80"/>')
    + para(ids, run('Elever: 412', '<w:sz w:val="20"/><w:szCs w:val="20"/>', ids), ppr='<w:spacing w:after="40"/>')
    + para(ids, run('Byggår: 1968, tillbyggt 2004', '<w:sz w:val="20"/><w:szCs w:val="20"/>', ids), ppr='<w:spacing w:after="40"/>')
    + para(ids, run('Uppvärmning: fjärrvärme', '<w:sz w:val="20"/><w:szCs w:val="20"/>', ids), ppr='<w:spacing w:after="40"/>')
    + para(ids, run('Sopsortering: 5 fraktioner', '<w:sz w:val="20"/><w:szCs w:val="20"/>', ids), ppr='<w:spacing w:after="0"/>')
)
box = text_box(ids, BOX, 1500000, 1650000, pos_h('page', off=5900000), pos_v('paragraph', off=0),
               name='Faktaruta', fill='E8F1F5', line='156082', line_w=12700)

c = []
c.append(heading(1, first=True))
c.append(body(box, 'Den här rapporten handlar om hur mycket avfall och energi som Ekbackeskolan använder under en '
              'vanlig skolvecka. Syftet är att ta reda på var skolan kan spara mest med enkla åtgärder, och att '
              'föreslå vad eleverna själva kan göra.', fn_ref(1)))
c.append(body('Vi fick idén under en lektion i naturkunskap när vi pratade om klimatmålen. Skolans matsal slänger '
              'mycket mat, och vi undrade om det går att mäta hur mycket det egentligen rör sig om. Rapporten '
              'bygger på mätningar vi gjorde under fyra veckor i februari och mars.'))
c.append(body('Frågeställningarna är: hur mycket restavfall och matavfall uppstår per elev och dag, och hur stor '
              'del av elförbrukningen går åt när ingen är i byggnaden?'))
c.append(heading(2))
c.append(body('Svenska skolor står för en betydande del av kommunernas energianvändning.', fn_ref(2),
              ' Byggnader från 1960-talet är ofta dåligt isolerade och har ventilationssystem som går dygnet runt. '
              'Samtidigt har många skolor kommit långt med källsortering, även om matavfallet fortfarande är ett problem.', en_ref(1)))
c.append(body('Ekbackeskolan har sedan 2022 haft en miljögrupp med elever från åk 7 till 9. Gruppen har bland annat '
              'infört pantkorgar i korridorerna och släcker belysningen i de salar som inte används. Vi ville se om '
              'dessa åtgärder gett märkbar effekt.'))
c.append(heading(3))
c.append(body('Vi mätte under tjugo skoldagar. Varje morgon vägde vi det matavfall och restavfall som samlats '
              'dagen innan, och vi noterade elmätarens värde klockan 07.00 och 16.00. Fastighetsskötaren hjälpte oss att '
              'läsa av mätaren på kvällar och helger.', fn_ref(3)))
c.append(body('Mätningarna följde dessa steg:', jc=False, ppr='<w:keepNext/>'))
for txt in ['Matavfallet vägdes i sina kärl efter lunchen, före tömning.',
            'Restavfallet vägdes av vaktmästaren varje morgon och antecknades i ett protokoll.',
            'Elförbrukningen lästes av klockan 07.00, 16.00 och på söndagar klockan 12.00.']:
    c.append(para(ids, run(txt, ids=ids), style='Liststycke', ppr='<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>'))
c.append(body('Alla värden fördes in i ett kalkylblad. Vi räknade ut medelvärden per dag och per elev, och jämförde '
              'med uppgifter från kommunens miljöenhet.', en_ref(2)))
c.append(heading(4))


def table():
    def cell(w, txt, bold=False, shade=None, jc=None):
        tcpr = '<w:tcW w:w="%d" w:type="dxa"/>%s' % (w, '<w:shd w:val="clear" w:color="auto" w:fill="%s"/>' % shade if shade else '')
        rp = '<w:b/><w:bCs/>' if bold else ''
        return ('<w:tc><w:tcPr>%s</w:tcPr>%s</w:tc>'
                % (tcpr, para(ids, run(txt, rp, ids), ppr=('<w:jc w:val="%s"/>' % jc if jc else '') + '<w:spacing w:after="0"/>')))
    widths = [3200, 2227, 2227]
    rows = [('Mått', 'Genomsnitt per dag', 'Per elev'), ('Matavfall', '38 kg', '92 g'), ('Restavfall', '64 kg', '155 g'),
            ('El, skoldag', '1 480 kWh', '3,6 kWh'), ('El, söndag', '620 kWh', '–')]
    out = ('<w:tbl><w:tblPr><w:tblStyle w:val="Tabellrutnt"/><w:tblW w:w="%d" w:type="dxa"/><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" '
           'w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/></w:tblPr><w:tblGrid>%s</w:tblGrid>'
           % (sum(widths), ''.join('<w:gridCol w:w="%d"/>' % w for w in widths)))
    for r, row in enumerate(rows):
        out += '<w:tr w:rsidR="00A1B2C3"><w:trPr><w:cantSplit/></w:trPr>%s</w:tr>' % ''.join(
            cell(widths[i], t_, bold=(r == 0), shade='DCE6EB' if r == 0 else None, jc=('right' if i else None)) for i, t_ in enumerate(row))
    return out + '</w:tbl>'


c.append(body('Tabellen visar medelvärdena för de tjugo mätdagarna. Matavfallet var störst på måndagar och fredagar, '
              'då fler äter skollunch.', ppr='<w:keepNext/>'))
c.append(table())
c.append(para(ids, '', ppr='<w:spacing w:after="0"/>'))
c.append(body('Mest överraskande var elförbrukningen under söndagar: drygt 40 procent av en vanlig skoldags förbrukning '
              'går åt när ingen är i byggnaden. Största delen verkar komma från ventilationen och från datorer som '
              'står i viloläge i datasalen.'))
c.append(heading(5))
c.append(body('Resultaten visar att det finns tydliga möjligheter att minska både avfall och energi. Matavfallet '
              'är lättast att påverka eftersom det beror på hur mycket mat som portioneras upp.', fn_ref(4)))
c.append(body('Mätperioden var kort och februari är en kall månad, så siffrorna för el är förmodligen högre än '
              'årsmedelvärdet. Vi mätte dessutom bara vikt, inte volym, vilket gör att lätta material som plast och '
              'kartong underskattas.'))
c.append(heading(6))
c.append(body('Skolan kan spara både pengar och miljö genom att stänga av ventilationen nattetid, välja mindre '
              'portioner som standard och införa en enkel rutin för att stänga ned datorer på fredagseftermiddagen. '
              'Vi hoppas att miljögruppen kan fortsätta mäta nästa läsår.'))
c.append(heading(7))
c.append(body('Kommunens miljöenhet (2025). Avfallsstatistik för kommunala verksamheter. Intern rapport.', jc=False))
c.append(body('Energimyndigheten. Energistatistik för lokaler. Hämtad 2026-02-20.', jc=False))
c.append(body('Fastighetsskötarens mätprotokoll, Ekbackeskolan, februari–mars 2026.', jc=False))

final_sect = ('<w:sectPr w:rsidR="00A1B2C3"><w:headerReference w:type="even" r:id="rId8"/><w:headerReference w:type="default" r:id="rId9"/>'
              '<w:footerReference w:type="even" r:id="rId10"/><w:footerReference w:type="default" r:id="rId11"/>'
              '<w:headerReference w:type="first" r:id="rId12"/><w:footerReference w:type="first" r:id="rId13"/>'
              '<w:pgSz w:w="%d" w:h="%d"/><w:pgMar w:top="%d" w:right="%d" w:bottom="%d" w:left="%d" w:header="709" w:footer="709" w:gutter="0"/>'
              '<w:cols w:space="708"/><w:titlePg/><w:docGrid w:linePitch="360"/></w:sectPr>' % (W, H, MT, MR, MB, ML))

doc_xml = XML + '<w:document %s><w:body>%s%s%s%s</w:body></w:document>' % (NS_DECL, title_page, toc(), ''.join(c), final_sect)

# ---------------------------------------------------------------- sidhuvud/sidfot
HF_NS = NS_DECL


def part(tag, content):
    return XML + '<w:%s %s>%s</w:%s>' % (tag, HF_NS, content, tag)


BORDER_B = '<w:pBdr><w:bottom w:val="single" w:sz="6" w:space="4" w:color="156082"/></w:pBdr>'
# udda sidor: logga till vänster, rapportnamn till höger (högertabb)
header_odd = part('hdr', para(ids, inline_pic(ids, 'rId1', 'Skolans logga', 457200, 457200, 'Skolans logga, en blå cirkel med bokstaven E')
                              + '<w:r><w:tab/></w:r>' + run(REPORT, '<w:color w:val="595959"/><w:sz w:val="20"/><w:szCs w:val="20"/>', ids),
                              style='Sidhuvud', ppr=BORDER_B.replace('<w:pBdr>', '<w:pBdr>') and
                              '<w:pBdr><w:bottom w:val="single" w:sz="6" w:space="4" w:color="156082"/></w:pBdr>'
                              '<w:tabs><w:tab w:val="clear" w:pos="4536"/><w:tab w:val="clear" w:pos="9072"/><w:tab w:val="right" w:pos="%d"/></w:tabs>' % TEXT_W))


# jämna sidor: tabell utan ramar (som i Words mallar), rapportnamn till vänster och logga till höger
def hdr_table():
    def cell(w, content, jc):
        return ('<w:tc><w:tcPr><w:tcW w:w="%d" w:type="dxa"/><w:vAlign w:val="center"/></w:tcPr>%s</w:tc>'
                % (w, para(ids, content, style='Sidhuvud', ppr='<w:jc w:val="%s"/>' % jc)))
    return ('<w:tbl><w:tblPr><w:tblW w:w="%d" w:type="dxa"/><w:tblBorders><w:bottom w:val="single" w:sz="6" w:space="0" w:color="156082"/></w:tblBorders>'
            '<w:tblLook w:val="0000" w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="0"/></w:tblPr>'
            '<w:tblGrid><w:gridCol w:w="%d"/><w:gridCol w:w="%d"/></w:tblGrid><w:tr>%s%s</w:tr></w:tbl>'
            % (TEXT_W, TEXT_W - 900, 900,
               cell(TEXT_W - 900, run(REPORT, '<w:color w:val="595959"/><w:sz w:val="20"/><w:szCs w:val="20"/>', ids), 'left'),
               cell(900, inline_pic(ids, 'rId1', 'Skolans logga', 457200, 457200, 'Skolans logga, en blå cirkel med bokstaven E'), 'right')))


header_even = part('hdr', hdr_table() + para(ids, '', style='Sidhuvud'))
header_first = part('hdr', para(ids, '', style='Sidhuvud'))
footer_first = part('ftr', para(ids, '', style='Sidfot'))
RP = '<w:color w:val="595959"/><w:sz w:val="20"/><w:szCs w:val="20"/>'
# udda: fldSimple, jämna: fldChar-sekvens
footer_odd = part('ftr', para(ids, run('Sida ', RP, ids) + fld_simple('PAGE', '3', RP) + run(' av ', RP, ids) + fld_simple('NUMPAGES', '5', RP),
                              style='Sidfot', ppr='<w:jc w:val="right"/>'))
footer_even = part('ftr', para(ids, run('Sida ', RP, ids) + fld_complex('PAGE', '2', RP) + run(' av ', RP, ids) + fld_complex('NUMPAGES', '5', RP),
                               style='Sidfot'))

# ---------------------------------------------------------------- fotnoter och slutnoter
SEP = ('<w:%(k)s w:type="separator" w:id="-1"><w:p w14:paraId="0A0A0A01" w14:textId="77777777" w:rsidR="00A1B2C3" w:rsidRDefault="00A1B2C3"><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:%(k)s>'
       '<w:%(k)s w:type="continuationSeparator" w:id="0"><w:p w14:paraId="0A0A0A02" w14:textId="77777777" w:rsidR="00A1B2C3" w:rsidRDefault="00A1B2C3"><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:%(k)s>')


def note(kind, n, text, style, refstyle):
    mark = 'footnoteRef' if kind == 'footnote' else 'endnoteRef'
    return ('<w:%s w:id="%d"><w:p w14:paraId="%s" w14:textId="77777777" w:rsidR="00A1B2C3" w:rsidRDefault="00A1B2C3"><w:pPr><w:pStyle w:val="%s"/></w:pPr>'
            '<w:r><w:rPr><w:rStyle w:val="%s"/></w:rPr><w:%s/></w:r><w:r><w:t xml:space="preserve"> %s</w:t></w:r></w:p></w:%s>'
            % (kind, n, ids.para_id(), style, refstyle, mark, escape(text), kind))


footnotes = part('footnotes', SEP % {'k': 'footnote'} + ''.join(note('footnote', n, tx, 'Fotnotstext', 'Fotnotsreferens') for n, tx in [
    (1, 'Med skolvecka avses måndag till fredag, utan lov och studiedagar.'),
    (2, 'Se kommunens miljöenhet (2025), avsnitt 3 om energianvändning i skolor.'),
    (3, 'Mätaren sitter i källaren intill fjärrvärmecentralen och är inte plomberad.'),
    (4, 'Portionsstorlekarna bestäms av matsalspersonalen och kan ändras på en dag.')]))
endnotes = part('endnotes', SEP % {'k': 'endnote'} + ''.join(note('endnote', n, tx, 'Slutnotstext', 'Slutnotsreferens') for n, tx in [
    (1, 'Uppgiften bygger på en sammanställning av svenska skolors källsortering som vi läste i februari.'),
    (2, 'Jämförelsetalen kommer från kommunens miljöenhet och avser läsåret 2024/25.')]))

numbering = (XML + '<w:numbering %s><w:abstractNum w:abstractNumId="0" w15:restartNumberingAfterBreak="0"><w:multiLevelType w:val="hybridMultilevel"/>'
             '<w:lvl w:ilvl="0" w:tplc="041D0001"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="&#xF0B7;"/><w:lvlJc w:val="left"/>'
             '<w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol" w:hint="default"/></w:rPr></w:lvl>'
             '<w:lvl w:ilvl="1" w:tplc="041D0003"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="o"/><w:lvlJc w:val="left"/>'
             '<w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New" w:hint="default"/></w:rPr></w:lvl>'
             '</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>') % NS_DECL

EXTRA_STYLES = (
    '<w:style w:type="paragraph" w:styleId="Liststycke"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="34"/><w:qFormat/>'
    '<w:pPr><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Fotnotstext"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:link w:val="FotnotstextChar"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>'
    '<w:style w:type="character" w:customStyle="1" w:styleId="FotnotstextChar"><w:name w:val="Fotnotstext Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Fotnotstext"/><w:uiPriority w:val="99"/><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>'
    '<w:style w:type="character" w:styleId="Fotnotsreferens"><w:name w:val="footnote reference"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Slutnotstext"><w:name w:val="endnote text"/><w:basedOn w:val="Normal"/><w:link w:val="SlutnotstextChar"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>'
    '<w:style w:type="character" w:customStyle="1" w:styleId="SlutnotstextChar"><w:name w:val="Slutnotstext Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Slutnotstext"/><w:uiPriority w:val="99"/><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>'
    '<w:style w:type="character" w:styleId="Slutnotsreferens"><w:name w:val="endnote reference"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Innehllsfrteckningsrubrik"><w:name w:val="TOC Heading"/><w:basedOn w:val="Rubrik1"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:qFormat/>'
    '<w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Innehll1"><w:name w:val="toc 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:autoRedefine/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="100"/></w:pPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Innehll2"><w:name w:val="toc 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:autoRedefine/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="100"/><w:ind w:left="220"/></w:pPr></w:style>')

parts = {
    'docProps/app.xml': app_xml(5, 700, 4600),
    'docProps/core.xml': core_xml('Hållbar skola: avfall och energi på Ekbackeskolan', 'Elsa Karlsson'),
    '_rels/.rels': root_rels(),
    'word/document.xml': doc_xml,
    'word/_rels/document.xml.rels': rels([
        ('rId1', 'styles', 'styles.xml'), ('rId2', 'numbering', 'numbering.xml'), ('rId3', 'settings', 'settings.xml'),
        ('rId4', 'webSettings', 'webSettings.xml'), ('rId5', 'footnotes', 'footnotes.xml'), ('rId6', 'endnotes', 'endnotes.xml'),
        ('rId7', 'fontTable', 'fontTable.xml'), ('rId8', 'header', 'header2.xml'), ('rId9', 'header', 'header1.xml'),
        ('rId10', 'footer', 'footer2.xml'), ('rId11', 'footer', 'footer1.xml'), ('rId12', 'header', 'header3.xml'),
        ('rId13', 'footer', 'footer3.xml'), ('rId14', 'theme', 'theme/theme1.xml')]),
    'word/header1.xml': header_odd, 'word/header2.xml': header_even, 'word/header3.xml': header_first,
    'word/footer1.xml': footer_odd, 'word/footer2.xml': footer_even, 'word/footer3.xml': footer_first,
    'word/_rels/header1.xml.rels': rels([('rId1', 'image', 'media/image1.png')]),
    'word/_rels/header2.xml.rels': rels([('rId1', 'image', 'media/image1.png')]),
    'word/media/image1.png': logo_png(),
    'word/footnotes.xml': footnotes, 'word/endnotes.xml': endnotes, 'word/numbering.xml': numbering,
    'word/styles.xml': styles_xml(EXTRA_STYLES),
    'word/settings.xml': settings(even_odd=True, footnote_pr=True),
    'word/webSettings.xml': web_settings(), 'word/fontTable.xml': font_table(FONTS), 'word/theme/theme1.xml': theme_xml(),
}
cts = [('word/document.xml', CT['document']), ('word/numbering.xml', CT['numbering']), ('word/styles.xml', CT['styles']),
       ('word/settings.xml', CT['settings']), ('word/webSettings.xml', CT['webSettings']), ('word/footnotes.xml', CT['footnotes']),
       ('word/endnotes.xml', CT['endnotes']), ('word/fontTable.xml', CT['fontTable']), ('word/theme/theme1.xml', CT['theme']),
       ('docProps/core.xml', CT['core']), ('docProps/app.xml', CT['app'])]
cts += [('word/header%d.xml' % i, CT['header']) for i in (1, 2, 3)] + [('word/footer%d.xml' % i, CT['footer']) for i in (1, 2, 3)]

if __name__ == '__main__':
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    os.makedirs(out, exist_ok=True)
    write_docx(os.path.join(out, 'gen-skolrapport.docx'), parts, cts)
