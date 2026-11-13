import base64, struct, zlib, sys, os

def png(w, h, rgb):
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    def ch(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    return b'\x89PNG\r\n\x1a\n' + ch(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + ch(b'IDAT', zlib.compress(raw)) + ch(b'IEND', b'')

IMG = base64.b64encode(png(60, 40, (200, 60, 60))).decode()

NS = ('xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
      'xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" '
      'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
      'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
      'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
      'xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" '
      'xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:dc="http://purl.org/dc/elements/1.1/" '
      'xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" '
      'xmlns:math="http://www.w3.org/1998/Math/MathML" '
      'xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"')

def listlevel(kind, n, fmt, pos):
    props = ('<style:list-level-properties text:list-level-position-and-space-mode="label-alignment">'
             '<style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="%sin" fo:text-indent="-0.25in" fo:margin-left="%sin"/>'
             '</style:list-level-properties>') % (pos, pos)
    if kind == 'b':
        return '<text:list-level-style-bullet text:level="%d" text:bullet-char="%s">%s</text:list-level-style-bullet>' % (n, fmt, props)
    return '<text:list-level-style-number text:level="%d" style:num-suffix="%s" style:num-format="%s">%s</text:list-level-style-number>' % (n, fmt[1], fmt[0], props)

STYLES = '''<office:font-face-decls/>
<office:styles>
<style:style style:name="Standard" style:family="paragraph" style:class="text"/>
<style:style style:name="Heading_20_1" style:display-name="Heading 1" style:family="paragraph" style:next-style-name="Text_20_body" style:default-outline-level="1" style:class="text"><style:text-properties fo:font-size="18pt" fo:font-weight="bold"/></style:style>
<style:style style:name="Heading_20_2" style:display-name="Heading 2" style:family="paragraph" style:next-style-name="Text_20_body" style:default-outline-level="2" style:class="text"><style:text-properties fo:font-size="14pt" fo:font-weight="bold"/></style:style>
<style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard" style:class="text"/>
<style:style style:name="Header" style:family="paragraph" style:parent-style-name="Standard"/>
<style:style style:name="Footer" style:family="paragraph" style:parent-style-name="Standard"/>
<style:style style:name="Table_20_Contents" style:display-name="Table Contents" style:family="paragraph" style:parent-style-name="Standard"/>
<style:style style:name="Contents_20_1" style:display-name="Contents 1" style:family="paragraph" style:parent-style-name="Standard"/>
<style:style style:name="Footnote" style:family="paragraph" style:parent-style-name="Standard"/>
<text:list-style style:name="L1">''' + listlevel('b', 1, '•', '0.5') + listlevel('b', 2, '◦', '1') + '''</text:list-style>
<text:list-style style:name="L2">''' + listlevel('n', 1, ('1', '.'), '0.5') + listlevel('n', 2, ('a', ')'), '1') + '''</text:list-style>
<text:notes-configuration text:note-class="footnote" style:num-format="1" text:start-value="0" text:footnotes-position="page" text:start-numbering-at="document"/>
</office:styles>
<office:automatic-styles>
<style:style style:name="Tbl" style:family="table"><style:table-properties style:width="6in" table:align="margins"/></style:style>
<style:style style:name="Tbl.A" style:family="table-column"><style:table-column-properties style:column-width="2in"/></style:style>
<style:style style:name="Tbl.C" style:family="table-cell"><style:table-cell-properties fo:padding="0.04in" fo:border="0.5pt solid #000000"/></style:style>
<style:style style:name="Ital" style:family="text"><style:text-properties fo:font-style="italic"/></style:style>
<style:style style:name="Sect2" style:family="section"><style:section-properties><style:columns fo:column-count="2" fo:column-gap="0.25in"/></style:section-properties></style:style>
<style:style style:name="Fr" style:family="graphic"><style:graphic-properties style:wrap="none" text:anchor-type="as-char" style:vertical-pos="top" style:vertical-rel="baseline"/></style:style>
<style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="8.27in" fo:page-height="11.69in" fo:margin-top="0.8in" fo:margin-bottom="0.8in" fo:margin-left="0.9in" fo:margin-right="0.9in"/><style:header-style/><style:footer-style/></style:page-layout>
</office:automatic-styles>
'''

def doc(title, hf, body):
    return ('<?xml version="1.0" encoding="UTF-8"?>\n<office:document %s office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text">\n'
            '<office:meta><dc:title>%s</dc:title></office:meta>\n%s'
            '<office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1">%s</style:master-page></office:master-styles>\n'
            '<office:body><office:text>%s</office:text></office:body></office:document>') % (NS, title, STYLES, hf, body)

def cell(txt, extra=''):
    return '<table:table-cell table:style-name="Tbl.C" office:value-type="string"%s><text:p text:style-name="Table_20_Contents">%s</text:p></table:table-cell>' % (extra, txt)

def covered():
    return '<table:covered-table-cell table:style-name="Tbl.C"><text:p/></table:covered-table-cell>'

def table(rows):
    return ('<table:table table:name="T" table:style-name="Tbl"><table:table-column table:style-name="Tbl.A" table:number-columns-repeated="3"/>'
            + ''.join('<table:table-row>%s</table:table-row>' % r for r in rows) + '</table:table>')

H1 = lambda t: '<text:h text:style-name="Heading_20_1" text:outline-level="1">%s</text:h>' % t
H2 = lambda t: '<text:h text:style-name="Heading_20_2" text:outline-level="2">%s</text:h>' % t
P = lambda t: '<text:p text:style-name="Text_20_body">%s</text:p>' % t

HF = ('<style:header><text:p text:style-name="Header">Rapporthuvud</text:p></style:header>'
      '<style:footer><text:p text:style-name="Footer">Sida <text:page-number text:select-page="current">1</text:page-number> av <text:page-count>1</text:page-count></text:p></style:footer>')

docs = {}
docs['tabeller-sammanslagna'] = ('Tabeller med sammanslagna celler', '',
    H1('Tabeller') + P('Före tabellen.') + table([
        cell('Rubrik över två kolumner', ' table:number-columns-spanned="2"') + covered() + cell('C1'),
        cell('A2') + cell('B2 och B3', ' table:number-rows-spanned="2"') + cell('C2'),
        cell('A3') + covered() + cell('C3'),
        cell('A4') + cell('B4') + cell('C4')]) + P('Efter tabellen.'))

docs['fotnoter-kommentarer'] = ('Fotnoter och kommentarer', '',
    H1('Noter')
    + P('Första stycket har en fotnot<text:note text:id="ftn1" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p text:style-name="Footnote">Det här är fotnot ett.</text:p></text:note-body></text:note> och fortsätter.')
    + P('Andra stycket har en <office:annotation office:name="c1"><dc:creator>Anna</dc:creator><dc:date>2026-01-05T10:00:00</dc:date><text:p>En kommentar om ordet.</text:p></office:annotation>kommenterad<office:annotation-end office:name="c1"/> del och en andra fotnot<text:note text:id="ftn2" text:note-class="footnote"><text:note-citation>2</text:note-citation><text:note-body><text:p text:style-name="Footnote">Fotnot två med <text:span text:style-name="Ital">kursiv</text:span> text.</text:p></text:note-body></text:note>.'))

docs['spara-andringar'] = ('Spårade ändringar', '',
    '<text:tracked-changes text:track-changes="true">'
    '<text:changed-region text:id="ct1"><text:insertion><office:change-info><dc:creator>Bo</dc:creator><dc:date>2026-02-01T09:00:00</dc:date></office:change-info></text:insertion></text:changed-region>'
    '<text:changed-region text:id="ct2"><text:deletion><office:change-info><dc:creator>Bo</dc:creator><dc:date>2026-02-01T09:05:00</dc:date></office:change-info><text:p text:style-name="Text_20_body">raderad text </text:p></text:deletion></text:changed-region>'
    '</text:tracked-changes>' + H1('Granskning')
    + P('Vanlig text, <text:change-start text:change-id="ct1"/>infogad text<text:change-end text:change-id="ct1"/> och <text:change-start text:change-id="ct2"/><text:change-end text:change-id="ct2"/>slut.'))

docs['sidhuvud-sektioner'] = ('Sidhuvud, sidfot och sektioner', HF,
    H1('Första delen') + P('Text före sektionen.')
    + '<text:section text:style-name="Sect2" text:name="S2">' + P('Text i en sektion med två spalter. Lorem ipsum dolor amet, consectetur adipiscing, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.') + P('Andra stycket i sektionen.') + '</text:section>'
    + H2('Efter sektionen') + P('Slutstycke.'))

docs['bild-listor'] = ('Bilder och listor', '',
    H1('Bild')
    + P('En bild: <draw:frame draw:style-name="Fr" draw:name="Bild1" text:anchor-type="as-char" svg:width="1.2in" svg:height="0.8in"><draw:image xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad" draw:mime-type="image/png"><office:binary-data>' + IMG + '</office:binary-data></draw:image><svg:title>Röd ruta</svg:title></draw:frame> mitt i texten.')
    + H1('Listor')
    + '<text:list text:style-name="L1"><text:list-item>' + P('Punkt ett') + '<text:list><text:list-item>' + P('Underpunkt') + '</text:list-item></text:list></text:list-item><text:list-item>' + P('Punkt två') + '</text:list-item></text:list>'
    + '<text:list text:style-name="L2"><text:list-item>' + P('Första') + '<text:list><text:list-item>' + P('Delsteg a') + '</text:list-item><text:list-item>' + P('Delsteg b') + '</text:list-item></text:list></text:list-item><text:list-item>' + P('Andra') + '</text:list-item></text:list>')

M = lambda tag, t: '<math:%s>%s</math:%s>' % (tag, t, tag)
formula = ('<math:math><math:semantics><math:mrow>'
    + M('msup', M('mrow', M('mo', '(') + M('mi', 'a') + M('mo', '+') + M('mi', 'b') + M('mo', ')')) + M('mn', '2'))
    + M('mo', '=') + M('msup', M('mi', 'a') + M('mn', '2')) + M('mo', '+') + M('mn', '2') + M('mi', 'a') + M('mi', 'b') + M('mo', '+') + M('msup', M('mi', 'b') + M('mn', '2'))
    + '</math:mrow><math:annotation math:encoding="StarMath 5.0">(a+b)^2 = a^2 + 2ab + b^2</math:annotation></math:semantics></math:math>')
docs['innehall-formler'] = ('Innehållsförteckning och formler', '',
    '<text:table-of-content text:name="TOC"><text:table-of-content-source text:outline-level="2"/><text:index-body><text:index-title text:name="TOCT"><text:p text:style-name="Heading_20_1">Innehåll</text:p></text:index-title><text:p text:style-name="Contents_20_1">Inledning<text:tab/>1</text:p><text:p text:style-name="Contents_20_1">Formler<text:tab/>1</text:p></text:index-body></text:table-of-content>'
    + H1('Inledning') + P('Text före formeln.') + H1('Formler')
    + P('Kvadratsatsen: <draw:frame draw:style-name="Fr" draw:name="Formel1" text:anchor-type="as-char" svg:width="1.5in" svg:height="0.3in"><draw:object>' + formula + '</draw:object></draw:frame> slut.'))

out = sys.argv[1]
for n, (title, hf, body) in docs.items():
    with open(os.path.join(out, n + '.fodt'), 'w', encoding='utf-8') as f:
        f.write(doc(title, hf, body))
