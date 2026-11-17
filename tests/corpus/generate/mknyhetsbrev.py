"""Nyhetsbrev i Word 365-stil: rubrik över hela bredden, löpande avsnitt med 2 spalter (w:sep), spaltbrytning,
bild i spalten, textruta (mc:AlternateContent: wps i Choice, VML i Fallback) förankrad med wp:anchor och
fyrkantsomflöde, samt ett avslutande avsnitt med 3 spalter av olika bredd (w:equalWidth="0" + w:col).
Allt är påhittat. Användning: python3 mknyhetsbrev.py <utkatalog>  ->  gen-nyhetsbrev.docx"""
import sys, os
from wordgen import *

ids = Ids()
W, H = 11906, 16838
MAR = 1134
TEXT_W = W - 2 * MAR   # 9638


def sect(kind=None, cols='<w:cols w:space="708"/>', last=False):
    typ = '<w:type w:val="%s"/>' % kind if kind else ''
    rs = ' w:rsidR="00A1B2C3" w:rsidSect="00D4E5F6"' if not last else ' w:rsidR="00D4E5F6" w:rsidSect="00D4E5F6"'
    return ('<w:sectPr%s>%s<w:pgSz w:w="%d" w:h="%d"/><w:pgMar w:top="%d" w:right="%d" w:bottom="%d" w:left="%d" '
            'w:header="709" w:footer="709" w:gutter="0"/>%s<w:docGrid w:linePitch="360"/></w:sectPr>'
            % (rs, typ, W, H, MAR, MAR, MAR, MAR, cols))


def body(text, **kw):
    return para(ids, run(text, ids=ids), ppr='<w:jc w:val="both"/>' if kw.get('just', True) else '')


def h(text, lvl=2):
    return para(ids, run(text, ids=ids), style='Rubrik%d' % lvl)


def img_bytes_landscape():
    # Påhittat landskap: himmel, sol, kullar och ett hus.
    def px(x, y):
        if y < 150:
            if (x - 480) ** 2 + (y - 60) ** 2 < 30 ** 2:
                return (250, 214, 90)
            return (150 + y // 3, 195 + y // 5, 235)
        hill = 190 + int(25 * (1 if (x // 90) % 2 else 0.4)) - (x % 90) // 12
        if y > hill:
            return (70 + (x * y) % 17, 140 + (y // 4) % 30, 70)
        return (110, 175, 105)
    def pix(x, y):
        if 120 <= x <= 230 and 150 <= y <= 235:
            if 150 <= x <= 175 and 195 <= y <= 235:
                return (92, 58, 38)
            return (178, 62, 52)
        if 105 <= x <= 245 and 112 <= y <= 150 and (y - 112) >= abs(x - 175) * 0.5 - 0 and False:
            return (60, 40, 40)
        if 100 <= x <= 250 and 110 <= y < 150 and (150 - y) <= (75 - abs(x - 175)) * 0.55 + 4 and (150 - y) >= 0:
            return (70, 50, 48)
        return px(x, y)
    return png(600, 360, lambda x, y: pix(x, y * 360 // 360) if y < 300 else (80, 150, 78))


TITLE = 'Björkbacken Nytt'
SUB = 'Nyhetsbrev från Brf Björkbacken · nummer 3 · mars 2026'

# ---------------------------------------------------------------- avsnitt 1: rubrik över hela bredden
s1 = (para(ids, run(TITLE, ids=ids), style='Rubrik',
           ppr='<w:pBdr><w:bottom w:val="single" w:sz="12" w:space="4" w:color="156082" w:themeColor="accent1"/></w:pBdr>')
      + para(ids, run(SUB, ids=ids), style='Underrubrik', ppr='<w:spacing w:after="240"/>'
             + sect(cols='<w:cols w:space="708"/>')))

# ---------------------------------------------------------------- avsnitt 2: två spalter med skiljelinje
COL2_W = (TEXT_W - 567) // 2   # 4535 twips
col1 = (
    h('Året som gått')
    + body('Styrelsen har hållit sex protokollförda möten sedan årsstämman i november. Mest tid har gått åt till '
           'upphandlingen av det nya taket över garagelängan, där tre offerter jämfördes under hösten. Valet föll på '
           'det företag som kunde börja redan i april och som lämnade tio års garanti.')
    + body('Fjärrvärmen blev något dyrare än budgeterat, men tack vare att vi sänkte framledningstemperaturen i '
           'trapphusen redan i oktober landade förbrukningen fyra procent under fjolårets. Tack till er som '
           'hjälpte till att vädra på rätt tider.')
    + body('Medlemsavgiften ligger kvar oförändrad under hela året. Underhållsplanen för de kommande tio åren '
           'finns att läsa på anslagstavlan i tvättstugan och i pärmen hos ordföranden.')
    + h('Ny lekplats på gården')
    + body('Efter flera önskemål från barnfamiljerna har styrelsen beslutat att byta ut sandlådan och de gamla '
           'gungorna. Den nya lekplatsen får klätterställning, två gungor och en liten kulle med rutschkana. '
           'Arbetet påbörjas i maj och beräknas vara klart före midsommar.')
    + body('Ritningen visar att björkarna vid cykelstället sparas. Några buskar längs staketet måste dock tas bort '
           'och ersätts med nya längre fram.')
    + para(ids, run('Frågor om lekplatsen kan ställas till Maja Lindqvist i styrelsen.', ids=ids)
           + '<w:r><w:br w:type="column"/></w:r>', ppr='<w:jc w:val="both"/>')
)

pic_para = para(ids, inline_pic(ids, 'rId8', 'Bild 1', 2700000, 1620000, 'Teckning av gården med ett rött hus och björkar'),
                ppr='<w:keepNext/><w:jc w:val="center"/><w:spacing w:after="60"/>')
caption = para(ids, run('Så här ser gården ut en vårdag. Teckning: Olle, 9 år.', 'i/' and '<w:i/><w:iCs/><w:color w:val="595959"/><w:sz w:val="18"/><w:szCs w:val="18"/>', ids),
               ppr='<w:jc w:val="center"/>')

BOX_INNER = (
    para(ids, run('Städdag lördag 25 april', '<w:b/><w:bCs/><w:color w:val="7F6000"/>', ids), ppr='<w:spacing w:after="60"/>')
    + para(ids, run('Samling vid cykelstället kl. 10. Föreningen bjuder på korv och saft.', '<w:sz w:val="20"/><w:szCs w:val="20"/>', ids), ppr='<w:spacing w:after="60"/>')
    + para(ids, run('Ta gärna med egna handskar och en kratta.', '<w:sz w:val="20"/><w:szCs w:val="20"/>', ids), ppr='<w:spacing w:after="0"/>')
)
box = text_box(ids, BOX_INNER, 1500000, 1330000, pos_h('column', align='right'), pos_v('paragraph', off=60000),
               name='Textruta', fill='FFF2CC', line='BF9000')

col2 = (
    pic_para + caption
    + h('Vårens aktiviteter')
    + para(ids, box + run('Vårens städdag är en av årets viktigaste samlingar. Vi rensar rabatter, sopar '
                          'cykelrummet och målar om bänkarna vid grillplatsen. Alla hushåll uppmanas delta, '
                          'men även en halvtimme gör skillnad.', ids=ids), ppr='<w:jc w:val="both"/>')
    + body('Kvällen efter städdagen är det grillning på gården. Anmäl er till Ture på tvättstugans anslagstavla '
           'senast den 20 april, så att vi vet hur mycket mat som behövs.')
    + body('I början av juni arrangerar bokcirkeln en loppis i föreningslokalen. Den som vill ha ett bord '
           'hör av sig till bokcirkelns kontaktperson under tisdagskvällarna.')
    + h('Tvättstugan')
    + body('Bokningssystemet har bytts ut och fungerar nu med den tagg som finns till porten. Har du tappat bort din '
           'tagg går det bra att beställa en ny hos fastighetsskötaren mot en avgift på 150 kronor.')
    + body('Kom ihåg att rengöra luddfiltret efter varje torktumling. Det sparar el och minskar brandrisken.')
)

sec2_end = para(ids, '', ppr=sect('continuous', '<w:cols w:num="2" w:sep="1" w:space="567"/>'))

# ---------------------------------------------------------------- avsnitt 3: tre spalter med olika bredd
c3 = (
    h('Kort och gott', 2)
    + body('Årsstämman hålls torsdagen den 14 maj klockan 18.30 i föreningslokalen. Kallelse delas ut i brevlådorna '
           'senast tre veckor innan.')
    + body('Pantbanken är flyttad till garagets bortre ände. Elbilsladdarna är nu installerade på plats 1–4; '
           'intresseanmälan för fler platser tas emot av styrelsen.')
    + body('Sopsorteringen fungerar bra, men kartong hamnar fortfarande i restavfallet. Platta gärna till kartongerna '
           'innan de slängs.')
    + h('Kontakt', 3)
    + body('Ordförande: Anna Sundström, trapphus A, lägenhet 12. Fastighetsskötare: Gunnar Ek, telefon 070-555 01 23, '
           'telefontid vardagar 7–9. Felanmälan utanför kontorstid går till jourtelefonen på anslagstavlan.')
    + body('Nästa nummer av Björkbacken Nytt kommer i juni. Skicka gärna in bidrag senast den 25 maj till '
           'styrelsens brevlåda i entrén.')
    + h('Tack!', 3)
    + body('Ett stort tack till alla som var med och skottade och sandade under den kalla perioden i februari. '
           'Ni gjorde att gångarna aldrig blev farliga.')
)
final_sect = sect('continuous', '<w:cols w:num="3" w:sep="1" w:space="360" w:equalWidth="0">'
                  '<w:col w:w="3600" w:space="360"/><w:col w:w="3200" w:space="360"/><w:col w:w="2118"/></w:cols>', last=True)

doc_xml = (XML + '<w:document %s><w:body>%s%s%s%s%s%s</w:body></w:document>'
           % (NS_DECL, s1, col1, col2, sec2_end, c3, final_sect))

parts = {
    'docProps/app.xml': app_xml(2, 520, 3400),
    'docProps/core.xml': core_xml('Björkbacken Nytt, nummer 3', 'Styrelsen Brf Björkbacken'),
    '_rels/.rels': root_rels(),
    'word/document.xml': doc_xml,
    'word/_rels/document.xml.rels': rels([
        ('rId1', 'styles', 'styles.xml'), ('rId2', 'settings', 'settings.xml'), ('rId3', 'webSettings', 'webSettings.xml'),
        ('rId4', 'fontTable', 'fontTable.xml'), ('rId5', 'theme', 'theme/theme1.xml'), ('rId8', 'image', 'media/image1.png')]),
    'word/media/image1.png': img_bytes_landscape(),
    'word/styles.xml': styles_xml(),
    'word/settings.xml': settings(even_odd=False, footnote_pr=False),
    'word/webSettings.xml': web_settings(),
    'word/fontTable.xml': font_table(FONTS),
    'word/theme/theme1.xml': theme_xml(),
}
cts = [('word/document.xml', CT['document']), ('word/styles.xml', CT['styles']), ('word/settings.xml', CT['settings']),
       ('word/webSettings.xml', CT['webSettings']), ('word/fontTable.xml', CT['fontTable']), ('word/theme/theme1.xml', CT['theme']),
       ('docProps/core.xml', CT['core']), ('docProps/app.xml', CT['app'])]

if __name__ == '__main__':
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    os.makedirs(out, exist_ok=True)
    write_docx(os.path.join(out, 'gen-nyhetsbrev.docx'), parts, cts)
