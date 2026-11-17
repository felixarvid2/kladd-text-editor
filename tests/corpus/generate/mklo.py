"""Nyhetsbrevet och skolrapporten som LibreOffice skapar dem: platt ODF (.fodt) med spalter, textruta
(draw:frame + draw:text-box), sidhuvud med bild, text:page-number/text:page-count och annorlunda första sida.
Användning: python3 mklo.py <utmapp>  ->  nyhetsbrev-lo.fodt och skolrapport-lo.fodt.
Konverteringen görs sedan med soffice (se SOURCES.md eller bygg.sh)."""
import base64, os, sys
from mknyhetsbrev import img_bytes_landscape
from mkskolrapport import logo_png

NS = ('xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" '
      'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
      'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" '
      'xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:dc="http://purl.org/dc/elements/1.1/" '
      'xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"')


def b64(data):
    return base64.b64encode(data).decode()


def image(name, data, w, h, anchor='as-char', style='Fr'):
    return ('<draw:frame draw:style-name="%s" draw:name="%s" text:anchor-type="%s" svg:width="%s" svg:height="%s">'
            '<draw:image xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad" draw:mime-type="image/png">'
            '<office:binary-data>%s</office:binary-data></draw:image></draw:frame>' % (style, name, anchor, w, h, b64(data)))


FONTS = '<office:font-face-decls><style:font-face style:name="Carlito" svg:font-family="Carlito" style:font-family-generic="swiss"/></office:font-face-decls>'

BASE_STYLES = '''<office:styles>
<style:default-style style:family="paragraph"><style:paragraph-properties fo:orphans="2" fo:widows="2"/><style:text-properties style:font-name="Carlito" fo:font-size="11pt" fo:language="sv" fo:country="SE"/></style:default-style>
<style:style style:name="Standard" style:family="paragraph" style:class="text"/>
<style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard" style:class="text"><style:paragraph-properties fo:margin-top="0in" fo:margin-bottom="0.1in" fo:text-align="justify"/></style:style>
<style:style style:name="Plain" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:text-align="start"/></style:style>
<style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body" style:class="text"><style:paragraph-properties fo:margin-top="0.17in" fo:margin-bottom="0.06in" fo:keep-with-next="always"/></style:style>
<style:style style:name="Heading_20_1" style:display-name="Heading 1" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Text_20_body" style:default-outline-level="1" style:class="text"><style:text-properties fo:font-size="20pt" fo:color="#0f4761"/></style:style>
<style:style style:name="Heading_20_2" style:display-name="Heading 2" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Text_20_body" style:default-outline-level="2" style:class="text"><style:text-properties fo:font-size="16pt" fo:color="#0f4761"/></style:style>
<style:style style:name="Heading_20_3" style:display-name="Heading 3" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Text_20_body" style:default-outline-level="3" style:class="text"><style:text-properties fo:font-size="14pt" fo:color="#0f4761"/></style:style>
<style:style style:name="Title" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Subtitle" style:class="chapter"><style:paragraph-properties fo:margin-bottom="0.05in" fo:border-bottom="0.75pt solid #156082" fo:padding="0in" style:join-border="false"/><style:text-properties fo:font-size="28pt"/></style:style>
<style:style style:name="Subtitle" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body" style:class="chapter"><style:paragraph-properties fo:margin-top="0.04in" fo:margin-bottom="0.12in"/><style:text-properties fo:font-size="14pt" fo:color="#595959"/></style:style>
<style:style style:name="Caption" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:paragraph-properties fo:text-align="center"/><style:text-properties fo:font-size="9pt" fo:font-style="italic" fo:color="#595959"/></style:style>
<style:style style:name="Header" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:paragraph-properties><style:tab-stops><style:tab-stop style:position="5.9in" style:type="right"/></style:tab-stops></style:paragraph-properties></style:style>
<style:style style:name="Footer" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"/>
<style:style style:name="Footnote" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:paragraph-properties fo:margin-left="0.2in" fo:text-indent="-0.2in"/><style:text-properties fo:font-size="10pt"/></style:style>
<style:style style:name="Endnote" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:paragraph-properties fo:margin-left="0.2in" fo:text-indent="-0.2in"/><style:text-properties fo:font-size="10pt"/></style:style>
<style:style style:name="Contents_20_Heading" style:display-name="Contents Heading" style:family="paragraph" style:parent-style-name="Heading_20_1" style:class="index"/>
<style:style style:name="Contents_20_1" style:display-name="Contents 1" style:family="paragraph" style:parent-style-name="Standard" style:class="index"><style:paragraph-properties fo:margin-bottom="0.06in"><style:tab-stops><style:tab-stop style:position="5.9in" style:type="right" style:leader-style="dotted" style:leader-text="."/></style:tab-stops></style:paragraph-properties></style:style>
<style:style style:name="Table_20_Contents" style:display-name="Table Contents" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"/>
<style:style style:name="Frame_20_contents" style:display-name="Frame contents" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"/>
<style:style style:name="Footnote_20_Symbol" style:display-name="Footnote Symbol" style:family="text"/>
<style:style style:name="Footnote_20_anchor" style:display-name="Footnote anchor" style:family="text"><style:text-properties style:text-position="super 58%"/></style:style>
<style:style style:name="Endnote_20_Symbol" style:display-name="Endnote Symbol" style:family="text"/>
<style:style style:name="Endnote_20_anchor" style:display-name="Endnote anchor" style:family="text"><style:text-properties style:text-position="super 58%"/></style:style>
<style:style style:name="Internet_20_link" style:display-name="Internet link" style:family="text"><style:text-properties fo:color="#467886" style:text-underline-style="solid"/></style:style>
<text:list-style style:name="L1"><text:list-level-style-bullet text:level="1" text:bullet-char="•"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="0.5in" fo:text-indent="-0.25in" fo:margin-left="0.5in"/></style:list-level-properties></text:list-level-style-bullet></text:list-style>
<text:notes-configuration text:note-class="footnote" style:num-format="1" text:start-value="0" text:footnotes-position="page" text:start-numbering-at="document"/>
<text:notes-configuration text:note-class="endnote" style:num-format="i" text:start-value="0"/>
</office:styles>'''


def doc(title, auto, masters, body):
    return ('<?xml version="1.0" encoding="UTF-8"?>\n<office:document %s office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text">\n'
            '<office:meta><dc:title>%s</dc:title><meta:generator>LibreOffice/24.2$Linux_X86_64</meta:generator></office:meta>\n%s%s\n'
            '<office:automatic-styles>%s</office:automatic-styles>\n<office:master-styles>%s</office:master-styles>\n'
            '<office:body><office:text>%s</office:text></office:body></office:document>') % (NS, title, FONTS, BASE_STYLES, auto, masters, body)


P = lambda t, st='Text_20_body': '<text:p text:style-name="%s">%s</text:p>' % (st, t)
H = lambda t, n=2: '<text:h text:style-name="Heading_20_%d" text:outline-level="%d">%s</text:h>' % (n, n, t)
FR = ('<style:style style:name="Fr" style:family="graphic"><style:graphic-properties style:wrap="none" text:anchor-type="as-char" '
      'style:vertical-pos="top" style:vertical-rel="baseline"/></style:style>')

# ------------------------------------------------------------------ nyhetsbrevet
def newsletter():
    cols = lambda n, gap, rel: ('<style:columns fo:column-count="%d" fo:column-gap="%s"><style:column-sep style:style="solid" style:width="0.0069in" '
                                'style:height="100%%" style:color="#000000"/>%s</style:columns>'
                                % (n, gap, ''.join('<style:column style:rel-width="%d*" fo:start-padding="%s" fo:end-padding="%s"/>' % (w, s, e) for w, s, e in rel)))
    auto = (FR
            + '<style:style style:name="Sect2" style:family="section"><style:section-properties text:dont-balance-text-columns="false" style:editable="false">'
            + cols(2, '0.3937in', [(4535, '0in', '0.1969in'), (4535, '0.1969in', '0in')]) + '</style:section-properties></style:style>'
            + '<style:style style:name="Sect3" style:family="section"><style:section-properties text:dont-balance-text-columns="false" style:editable="false">'
            + cols(3, '0.25in', [(3600, '0in', '0.125in'), (3200, '0.125in', '0.125in'), (2118, '0.125in', '0in')]) + '</style:section-properties></style:style>'
            + '<style:style style:name="PCol" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:break-before="column"/></style:style>'
            + '<style:style style:name="PPic" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-bottom="0.03in" fo:text-align="center" fo:break-before="column"/></style:style>'
            + '<style:style style:name="Box" style:family="graphic" style:parent-style-name="Frame"><style:graphic-properties svg:width="1.6in" style:wrap="parallel" style:number-wrapped-paragraphs="no-limit" '
              'style:wrap-contour="false" style:vertical-pos="top" style:vertical-rel="paragraph" style:horizontal-pos="right" style:horizontal-rel="paragraph-content" '
              'fo:margin-left="0.0799in" fo:margin-right="0in" fo:margin-top="0.04in" fo:margin-bottom="0.04in" fo:padding="0.0709in" fo:border="1.5pt solid #bf9000" '
              'fo:background-color="#fff2cc" draw:fill="solid" draw:fill-color="#fff2cc" style:shadow="none"/></style:style>'
            + '<style:style style:name="Bold" style:family="text"><style:text-properties fo:font-weight="bold" fo:color="#7f6000"/></style:style>'
            + '<style:style style:name="Small" style:family="paragraph" style:parent-style-name="Frame_20_contents"><style:paragraph-properties fo:margin-bottom="0.04in"/><style:text-properties fo:font-size="10pt"/></style:style>'
            + '<style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="8.2681in" fo:page-height="11.6929in" fo:margin-top="0.7874in" '
              'fo:margin-bottom="0.7874in" fo:margin-left="0.7874in" fo:margin-right="0.7874in"/><style:header-style/><style:footer-style/></style:page-layout>')
    masters = '<style:master-page style:name="Standard" style:page-layout-name="pm1"/>'
    box = ('<draw:frame draw:style-name="Box" draw:name="Textruta1" text:anchor-type="paragraph" svg:width="1.6in" draw:z-index="0">'
           '<draw:text-box fo:min-height="0.5in">'
           + P('Städdag lördag 25 april', 'Small').replace('Städdag lördag 25 april', '<text:span text:style-name="Bold">Städdag lördag 25 april</text:span>')
           + P('Samling vid cykelstället kl. 10. Föreningen bjuder på korv och saft.', 'Small')
           + P('Ta gärna med egna handskar och en kratta.', 'Small') + '</draw:text-box></draw:frame>')
    body = (
        P('Björkbacken Nytt', 'Title') + P('Nyhetsbrev från Brf Björkbacken · nummer 3 · mars 2026', 'Subtitle')
        + '<text:section text:style-name="Sect2" text:name="Spalter">'
        + H('Året som gått')
        + P('Styrelsen har hållit sex protokollförda möten sedan årsstämman i november. Mest tid har gått åt till upphandlingen av det nya taket över garagelängan, där tre offerter jämfördes under hösten. Valet föll på det företag som kunde börja redan i april och som lämnade tio års garanti.')
        + P('Fjärrvärmen blev något dyrare än budgeterat, men tack vare att vi sänkte framledningstemperaturen i trapphusen redan i oktober landade förbrukningen fyra procent under fjolårets. Tack till er som hjälpte till att vädra på rätt tider.')
        + P('Medlemsavgiften ligger kvar oförändrad under hela året. Underhållsplanen för de kommande tio åren finns att läsa på anslagstavlan i tvättstugan och i pärmen hos ordföranden.')
        + H('Ny lekplats på gården')
        + P('Efter flera önskemål från barnfamiljerna har styrelsen beslutat att byta ut sandlådan och de gamla gungorna. Den nya lekplatsen får klätterställning, två gungor och en liten kulle med rutschkana. Arbetet påbörjas i maj och beräknas vara klart före midsommar.')
        + P('Ritningen visar att björkarna vid cykelstället sparas. Några buskar längs staketet måste dock tas bort och ersätts med nya längre fram.')
        + P('Frågor om lekplatsen kan ställas till Maja Lindqvist i styrelsen.')
        + '<text:p text:style-name="PPic">' + image('Bild1', img_bytes_landscape(), '2.8in', '1.68in') + '</text:p>'
        + P('Så här ser gården ut en vårdag. Teckning: Olle, 9 år.', 'Caption')
        + H('Vårens aktiviteter')
        + P(box + 'Vårens städdag är en av årets viktigaste samlingar. Vi rensar rabatter, sopar cykelrummet och målar om bänkarna vid grillplatsen. Alla hushåll uppmanas delta, men även en halvtimme gör skillnad.')
        + P('Kvällen efter städdagen är det grillning på gården. Anmäl er till Ture på tvättstugans anslagstavla senast den 20 april, så att vi vet hur mycket mat som behövs.')
        + P('I början av juni arrangerar bokcirkeln en loppis i föreningslokalen. Den som vill ha ett bord hör av sig till bokcirkelns kontaktperson under tisdagskvällarna.')
        + H('Tvättstugan')
        + P('Bokningssystemet har bytts ut och fungerar nu med den tagg som finns till porten. Har du tappat bort din tagg går det bra att beställa en ny hos fastighetsskötaren mot en avgift på 150 kronor.')
        + P('Kom ihåg att rengöra luddfiltret efter varje torktumling. Det sparar el och minskar brandrisken.')
        + '</text:section>'
        + '<text:section text:style-name="Sect3" text:name="Tre_spalter">'
        + H('Kort och gott')
        + P('Årsstämman hålls torsdagen den 14 maj klockan 18.30 i föreningslokalen. Kallelse delas ut i brevlådorna senast tre veckor innan.')
        + P('Pantbanken är flyttad till garagets bortre ände. Elbilsladdarna är nu installerade på plats 1–4; intresseanmälan för fler platser tas emot av styrelsen.')
        + P('Sopsorteringen fungerar bra, men kartong hamnar fortfarande i restavfallet. Platta gärna till kartongerna innan de slängs.')
        + H('Kontakt', 3)
        + P('Ordförande: Anna Sundström, trapphus A, lägenhet 12. Fastighetsskötare: Gunnar Ek, telefon 070-555 01 23, telefontid vardagar 7–9. Felanmälan utanför kontorstid går till jourtelefonen på anslagstavlan.')
        + P('Nästa nummer av Björkbacken Nytt kommer i juni. Skicka gärna in bidrag senast den 25 maj till styrelsens brevlåda i entrén.')
        + H('Tack!', 3)
        + P('Ett stort tack till alla som var med och skottade och sandade under den kalla perioden i februari. Ni gjorde att gångarna aldrig blev farliga.')
        + '</text:section>')
    return doc('Björkbacken Nytt, nummer 3', auto, masters, body)


# ------------------------------------------------------------------ skolrapporten
def report():
    REPORT = 'Hållbar skola: avfall och energi på Ekbackeskolan'
    logo = lambda: image('Logga', logo_png(), '0.5in', '0.5in')
    hdr_tabs = '<text:tab/>'
    fn = [0]

    def note(kind, text):
        fn[0] += 1
        return ('<text:note text:id="%s%d" text:note-class="%s"><text:note-citation>%s</text:note-citation><text:note-body>%s</text:note-body></text:note>'
                % (kind[:3], fn[0], kind, fn[0] if kind == 'footnote' else 'i', P(text, 'Footnote' if kind == 'footnote' else 'Endnote')))
    N = lambda k, t: note(k, t)
    auto = (FR
            + '<style:style style:name="PBreak" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:break-after="page"/></style:style>'
            + '<style:style style:name="PTitle" style:family="paragraph" style:parent-style-name="Title"><style:paragraph-properties fo:margin-top="2.2in" fo:border-bottom="none"/></style:style>'
            + '<style:style style:name="PFirst" style:family="paragraph" style:parent-style-name="Standard" style:master-page-name="Standard"/>'
            + '<style:style style:name="PHead1" style:family="paragraph" style:parent-style-name="Heading_20_1"><style:paragraph-properties fo:break-before="page"/></style:style>'
            + '<style:style style:name="HdrBorder" style:family="paragraph" style:parent-style-name="Header"><style:paragraph-properties fo:border-bottom="0.75pt solid #156082" fo:padding-bottom="0.04in" style:join-border="false"/></style:style>'
            + '<style:style style:name="Box" style:family="graphic" style:parent-style-name="Frame"><style:graphic-properties svg:width="1.5in" style:wrap="parallel" style:number-wrapped-paragraphs="no-limit" '
              'style:vertical-pos="top" style:vertical-rel="paragraph" style:horizontal-pos="from-left" style:horizontal-rel="page" svg:x="6.45in" '
              'fo:padding="0.0591in" fo:border="1pt solid #156082" fo:background-color="#e8f1f5" draw:fill="solid" draw:fill-color="#e8f1f5" style:shadow="none"/></style:style>'
            + '<style:style style:name="Bold" style:family="text"><style:text-properties fo:font-weight="bold" fo:color="#0f4761"/></style:style>'
            + '<style:style style:name="Small" style:family="paragraph" style:parent-style-name="Frame_20_contents"><style:paragraph-properties fo:margin-bottom="0.03in"/><style:text-properties fo:font-size="10pt"/></style:style>'
            + '<style:style style:name="Tbl" style:family="table"><style:table-properties style:width="5.3in" table:align="left"/></style:style>'
            + '<style:style style:name="Tbl.A" style:family="table-column"><style:table-column-properties style:column-width="2.2in"/></style:style>'
            + '<style:style style:name="Tbl.B" style:family="table-column"><style:table-column-properties style:column-width="1.55in"/></style:style>'
            + '<style:style style:name="Tbl.H" style:family="table-cell"><style:table-cell-properties fo:padding="0.04in" fo:border="0.5pt solid #000000" fo:background-color="#dce6eb"/></style:style>'
            + '<style:style style:name="Tbl.C" style:family="table-cell"><style:table-cell-properties fo:padding="0.04in" fo:border="0.5pt solid #000000"/></style:style>'
            + '<style:style style:name="Right" style:family="paragraph" style:parent-style-name="Table_20_Contents"><style:paragraph-properties fo:text-align="end"/></style:style>'
            + '<style:style style:name="FooterR" style:family="paragraph" style:parent-style-name="Footer"><style:paragraph-properties fo:text-align="end"/></style:style>'
            + '<style:style style:name="Lst" style:family="paragraph" style:parent-style-name="Plain"/>'
            + '<style:page-layout style:name="pm1" style:page-usage="all"><style:page-layout-properties fo:page-width="8.2681in" fo:page-height="11.6929in" fo:margin-top="0.5in" '
              'fo:margin-bottom="0.5in" fo:margin-left="1.1in" fo:margin-right="1.9in"/><style:header-style><style:header-footer-properties fo:min-height="0.4in" fo:margin-bottom="0.15in"/></style:header-style>'
              '<style:footer-style><style:header-footer-properties fo:min-height="0.3in" fo:margin-top="0.15in"/></style:footer-style></style:page-layout>')
    # annorlunda första sida (header-first/footer-first) och olika udda/jämna sidor (header-left/footer-left)
    masters = ('<style:master-page style:name="Standard" style:page-layout-name="pm1">'
               '<style:header>' + P(logo() + hdr_tabs + REPORT, 'HdrBorder') + '</style:header>'
               '<style:header-left>' + P(REPORT + hdr_tabs + logo(), 'HdrBorder') + '</style:header-left>'
               '<style:header-first>' + P('', 'Header') + '</style:header-first>'
               '<style:footer>' + P('Sida <text:page-number text:select-page="current">1</text:page-number> av <text:page-count>1</text:page-count>', 'Footer') + '</style:footer>'
               '<style:footer-left>' + P('Sida <text:page-number text:select-page="current">1</text:page-number> av <text:page-count>1</text:page-count>', 'Footer') + '</style:footer-left>'
               '<style:footer-first>' + P('', 'Footer') + '</style:footer-first>'
               '</style:master-page>')
    toc_entries = [('Inledning', 3), ('Bakgrund', 3), ('Metod', 3), ('Resultat', 4), ('Diskussion', 4), ('Slutsats', 4), ('Källor', 4)]
    toc = ('<text:table-of-content text:style-name="Sect1" text:protected="false" text:name="Innehall"><text:table-of-content-source text:outline-level="3">'
           '</text:table-of-content-source><text:index-body><text:index-title text:style-name="Sect1" text:name="Innehall_Titel">'
           + P('Innehållsförteckning', 'Contents_20_Heading') + '</text:index-title>'
           + ''.join(P('%s<text:tab/>%d' % e, 'Contents_20_1') for e in toc_entries) + '</text:index-body></text:table-of-content>')
    toc = toc.replace('text:style-name="Sect1" ', '')
    box = ('<draw:frame draw:style-name="Box" draw:name="Faktaruta" text:anchor-type="paragraph" svg:x="6.45in" svg:width="1.5in" draw:z-index="0"><draw:text-box fo:min-height="0.5in">'
           + P('<text:span text:style-name="Bold">Fakta om skolan</text:span>', 'Small') + P('Elever: 412', 'Small') + P('Byggår: 1968, tillbyggt 2004', 'Small')
           + P('Uppvärmning: fjärrvärme', 'Small') + P('Sopsortering: 5 fraktioner', 'Small') + '</draw:text-box></draw:frame>')
    cell = lambda t, st='Tbl.C', ps='Table_20_Contents': '<table:table-cell table:style-name="%s" office:value-type="string"><text:p text:style-name="%s">%s</text:p></table:table-cell>' % (st, ps, t)
    rows = [('Mått', 'Genomsnitt per dag', 'Per elev'), ('Matavfall', '38 kg', '92 g'), ('Restavfall', '64 kg', '155 g'), ('El, skoldag', '1 480 kWh', '3,6 kWh'), ('El, söndag', '620 kWh', '–')]
    tbl = ('<table:table table:name="Resultat" table:style-name="Tbl"><table:table-column table:style-name="Tbl.A"/><table:table-column table:style-name="Tbl.B" table:number-columns-repeated="2"/>'
           + ''.join('<table:table-row>' + ''.join(cell(t, 'Tbl.H' if r == 0 else 'Tbl.C', 'Table_20_Contents' if i == 0 else 'Right') for i, t in enumerate(row)) + '</table:table-row>' for r, row in enumerate(rows))
           + '</table:table>')
    li = lambda t: '<text:list-item>' + P(t, 'Lst') + '</text:list-item>'
    body = (
        P('Hållbar skola', 'PTitle') + P('Avfall och energi på Ekbackeskolan', 'Subtitle')
        + P('', 'Standard').replace('<text:p', '<text:p', 1) * 6
        + P('<text:span text:style-name="Bold">Elsa Karlsson och Noah Berg</text:span>', 'Plain') + P('Klass 9B, Ekbackeskolan', 'Plain')
        + P('Handledare: Karin Öberg, lärare i naturkunskap', 'Plain') + P('Mars 2026', 'PBreak')
        + toc
        + '<text:h text:style-name="PHead1" text:outline-level="1">Inledning</text:h>'
        + P(box + 'Den här rapporten handlar om hur mycket avfall och energi som Ekbackeskolan använder under en vanlig skolvecka. Syftet är att ta reda på var skolan kan spara mest med enkla åtgärder, och att föreslå vad eleverna själva kan göra.'
            + N('footnote', 'Med skolvecka avses måndag till fredag, utan lov och studiedagar.'))
        + P('Vi fick idén under en lektion i naturkunskap när vi pratade om klimatmålen. Skolans matsal slänger mycket mat, och vi undrade om det går att mäta hur mycket det egentligen rör sig om. Rapporten bygger på mätningar vi gjorde under fyra veckor i februari och mars.')
        + P('Frågeställningarna är: hur mycket restavfall och matavfall uppstår per elev och dag, och hur stor del av elförbrukningen går åt när ingen är i byggnaden?')
        + H('Bakgrund', 1)
        + P('Svenska skolor står för en betydande del av kommunernas energianvändning.' + N('footnote', 'Se kommunens miljöenhet (2025), avsnitt 3 om energianvändning i skolor.')
            + ' Byggnader från 1960-talet är ofta dåligt isolerade och har ventilationssystem som går dygnet runt. Samtidigt har många skolor kommit långt med källsortering, även om matavfallet fortfarande är ett problem.'
            + N('endnote', 'Uppgiften bygger på en sammanställning av svenska skolors källsortering som vi läste i februari.'))
        + P('Ekbackeskolan har sedan 2022 haft en miljögrupp med elever från åk 7 till 9. Gruppen har bland annat infört pantkorgar i korridorerna och släcker belysningen i de salar som inte används. Vi ville se om dessa åtgärder gett märkbar effekt.')
        + H('Metod', 1)
        + P('Vi mätte under tjugo skoldagar. Varje morgon vägde vi det matavfall och restavfall som samlats dagen innan, och vi noterade elmätarens värde klockan 07.00 och 16.00. Fastighetsskötaren hjälpte oss att läsa av mätaren på kvällar och helger.'
            + N('footnote', 'Mätaren sitter i källaren intill fjärrvärmecentralen och är inte plomberad.'))
        + P('Mätningarna följde dessa steg:', 'Plain')
        + '<text:list text:style-name="L1">' + li('Matavfallet vägdes i sina kärl efter lunchen, före tömning.') + li('Restavfallet vägdes av vaktmästaren varje morgon och antecknades i ett protokoll.')
        + li('Elförbrukningen lästes av klockan 07.00, 16.00 och på söndagar klockan 12.00.') + '</text:list>'
        + P('Alla värden fördes in i ett kalkylblad. Vi räknade ut medelvärden per dag och per elev, och jämförde med uppgifter från kommunens miljöenhet.'
            + N('endnote', 'Jämförelsetalen kommer från kommunens miljöenhet och avser läsåret 2024/25.'))
        + H('Resultat', 1)
        + P('Tabellen visar medelvärdena för de tjugo mätdagarna. Matavfallet var störst på måndagar och fredagar, då fler äter skollunch.')
        + tbl + P('')
        + P('Mest överraskande var elförbrukningen under söndagar: drygt 40 procent av en vanlig skoldags förbrukning går åt när ingen är i byggnaden. Största delen verkar komma från ventilationen och från datorer som står i viloläge i datasalen.')
        + H('Diskussion', 1)
        + P('Resultaten visar att det finns tydliga möjligheter att minska både avfall och energi. Matavfallet är lättast att påverka eftersom det beror på hur mycket mat som portioneras upp.'
            + N('footnote', 'Portionsstorlekarna bestäms av matsalspersonalen och kan ändras på en dag.'))
        + P('Mätperioden var kort och februari är en kall månad, så siffrorna för el är förmodligen högre än årsmedelvärdet. Vi mätte dessutom bara vikt, inte volym, vilket gör att lätta material som plast och kartong underskattas.')
        + H('Slutsats', 1)
        + P('Skolan kan spara både pengar och miljö genom att stänga av ventilationen nattetid, välja mindre portioner som standard och införa en enkel rutin för att stänga ned datorer på fredagseftermiddagen. Vi hoppas att miljögruppen kan fortsätta mäta nästa läsår.')
        + H('Källor', 1)
        + P('Kommunens miljöenhet (2025). Avfallsstatistik för kommunala verksamheter. Intern rapport.', 'Plain')
        + P('Energimyndigheten. Energistatistik för lokaler. Hämtad 2026-02-20.', 'Plain')
        + P('Fastighetsskötarens mätprotokoll, Ekbackeskolan, februari–mars 2026.', 'Plain'))
    return doc('Hållbar skola: avfall och energi på Ekbackeskolan', auto, masters, body)


if __name__ == '__main__':
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    os.makedirs(out, exist_ok=True)
    for n, f in (('nyhetsbrev-lo', newsletter), ('skolrapport-lo', report)):
        with open(os.path.join(out, n + '.fodt'), 'w', encoding='utf-8') as fh:
            fh.write(f())
