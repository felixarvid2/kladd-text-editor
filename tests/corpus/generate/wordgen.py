"""Hjälpmodul: skriver .docx-paket med samma delar och namnrymder som Word 365 (document, styles med
docDefaults, settings med rsid, theme, fontTable, webSettings, docProps). Ingen bygger på python-docx;
allt är handskriven XML så att strukturen liknar Words egen utdata. Används av mknyhetsbrev.py och
mkskolrapport.py. Texterna och bilderna är påhittade."""
import io, struct, zlib, zipfile
from xml.sax.saxutils import escape

# Namnrymderna som Word 365 skriver på w:document (mc:Ignorable listar de som äldre läsare får hoppa över).
NS_DECL = (
    'xmlns:wpc="http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" '
    'xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" '
    'xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" '
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
    'xmlns:aink="http://schemas.microsoft.com/office/drawing/2016/ink" '
    'xmlns:am3d="http://schemas.microsoft.com/office/drawing/2017/model3d" '
    'xmlns:o="urn:schemas-microsoft-com:office:office" '
    'xmlns:oel="http://schemas.microsoft.com/office/2019/extlst" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
    'xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" '
    'xmlns:v="urn:schemas-microsoft-com:vml" '
    'xmlns:wp14="http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" '
    'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
    'xmlns:w10="urn:schemas-microsoft-com:office:word" '
    'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
    'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" '
    'xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" '
    'xmlns:w16cex="http://schemas.microsoft.com/office/word/2018/wordml/cex" '
    'xmlns:w16cid="http://schemas.microsoft.com/office/word/2016/wordml/cid" '
    'xmlns:w16="http://schemas.microsoft.com/office/word/2018/wordml" '
    'xmlns:w16du="http://schemas.microsoft.com/office/word/2023/wordml/word16du" '
    'xmlns:w16sdtdh="http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash" '
    'xmlns:w16sdtfl="http://schemas.microsoft.com/office/word/2024/wordml/sdtformatlock" '
    'xmlns:w16se="http://schemas.microsoft.com/office/word/2015/wordml/symex" '
    'xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" '
    'xmlns:wpi="http://schemas.microsoft.com/office/word/2010/wordprocessingInk" '
    'xmlns:wne="http://schemas.microsoft.com/office/word/2006/wordml" '
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" '
    'mc:Ignorable="w14 w15 w16se w16cid w16 w16cex w16sdtdh w16sdtfl w16du wp14"')

XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
RSID = ['00A1B2C3', '00D4E5F6', '00F1A2B3', '00C3D4E5']   # påhittade rsid-värden


def png(w, h, pixel):
    """Liten PNG utan bibliotek: pixel(x, y) -> (r, g, b)."""
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        for x in range(w):
            raw += bytes(pixel(x, y))
    def ch(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    return (b'\x89PNG\r\n\x1a\n' + ch(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
            + ch(b'IDAT', zlib.compress(bytes(raw), 9)) + ch(b'IEND', b''))


class Ids:
    """Löpnummer för w14:paraId, rsid-rotation och drawing-id."""
    def __init__(self):
        self.para = 0x1A2B3C00
        self.drawing = 0
        self.rs = 0
    def para_id(self):
        self.para += 0x1F3
        return '%08X' % (self.para & 0x7FFFFFFF)
    def rsid(self):
        self.rs += 1
        return RSID[self.rs % len(RSID)]
    def draw(self):
        self.drawing += 1
        return self.drawing


def t(text):
    return '<w:t xml:space="preserve">%s</w:t>' % escape(text)


def run(text, rpr='', ids=None):
    rs = ' w:rsidR="%s"' % ids.rsid() if ids else ''
    rp = '<w:rPr>%s</w:rPr>' % rpr if rpr else ''
    return '<w:r%s>%s%s</w:r>' % (rs, rp, t(text))


def para(ids, content, style=None, ppr='', rpr_mark=''):
    """Stycke med paraId/textId och rsid-attribut som Word skriver dem."""
    pid = ids.para_id()
    rs = ids.rsid()
    st = '<w:pStyle w:val="%s"/>' % style if style else ''
    mark = '<w:rPr>%s</w:rPr>' % rpr_mark if rpr_mark else ''
    pp = '<w:pPr>%s%s%s</w:pPr>' % (st, ppr, mark) if (st or ppr or mark) else ''
    return ('<w:p w14:paraId="%s" w14:textId="77777777" w:rsidR="%s" w:rsidRDefault="%s">%s%s</w:p>'
            % (pid, rs, rs, pp, content))


def fld_simple(instr, shown, rpr=''):
    rp = '<w:rPr>%s<w:noProof/></w:rPr>' % rpr
    return '<w:fldSimple w:instr=" %s "><w:r>%s<w:t>%s</w:t></w:r></w:fldSimple>' % (instr, rp, shown)


def fld_complex(instr, shown, rpr=''):
    rp = '<w:rPr>%s</w:rPr>' % rpr
    rpn = '<w:rPr>%s<w:noProof/></w:rPr>' % rpr
    return ('<w:r>%s<w:fldChar w:fldCharType="begin"/></w:r>'
            '<w:r>%s<w:instrText xml:space="preserve"> %s </w:instrText></w:r>'
            '<w:r>%s<w:fldChar w:fldCharType="separate"/></w:r>'
            '<w:r>%s<w:t>%s</w:t></w:r>'
            '<w:r>%s<w:fldChar w:fldCharType="end"/></w:r>') % (rp, rp, instr, rp, rpn, shown, rp)


EMU_PER_TWIP = 635


def pic_graphic(ids, rid, name, cx, cy, descr=''):
    """a:graphic med pic:pic (som Word skriver den) för en inbäddad bild."""
    n = ids.draw()
    return (
        '<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
        '<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        '<pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        '<pic:nvPicPr><pic:cNvPr id="%d" name="%s" descr="%s"/><pic:cNvPicPr/></pic:nvPicPr>'
        '<pic:blipFill><a:blip r:embed="%s"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        '<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="%d" cy="%d"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>'
        % (n, escape(name), escape(descr), rid, cx, cy))


def inline_pic(ids, rid, name, cx, cy, descr=''):
    n = ids.draw()
    return ('<w:r><w:rPr><w:noProof/></w:rPr><w:drawing>'
            '<wp:inline distT="0" distB="0" distL="0" distR="0" wp14:anchorId="%08X" wp14:editId="%08X">'
            '<wp:extent cx="%d" cy="%d"/><wp:effectExtent l="0" t="0" r="0" b="0"/>'
            '<wp:docPr id="%d" name="%s" descr="%s"/>'
            '<wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr>'
            '%s</wp:inline></w:drawing></w:r>'
            % (0x2C000000 + n, 0x3D000000 + n, cx, cy, n + 100, escape(name), escape(descr), pic_graphic(ids, rid, name, cx, cy, descr)))


def text_box(ids, inner, cx, cy, pos_h, pos_v, name='Textruta', fill='FFF2CC', line='BF9000', line_w=19050,
             wrap='square', insets=(91440, 45720, 91440, 45720)):
    """Textruta som Word 365 skriver den: mc:AlternateContent med wps:wsp i Choice och VML v:textbox i
    Fallback, båda med samma w:txbxContent. pos_h/pos_v är färdiga wp:positionH/-V-element.
    inner = innehållet i w:txbxContent (redan färdiga w:p)."""
    n = ids.draw()
    wrap_xml = {'square': '<wp:wrapSquare wrapText="bothSides"/>', 'none': '<wp:wrapNone/>',
                'topbottom': '<wp:wrapTopAndBottom/>'}[wrap]
    l, tp, r, b = insets
    # VML-fallbackens stil: samma mått i punkter (1 pt = 12700 EMU).
    pt = lambda e: '%.1fpt' % (e / 12700)
    def vml_pos(el, axis):
        rel = el.split('relativeFrom="')[1].split('"')[0]
        mapping = {'column': 'text', 'margin': 'margin', 'page': 'page', 'paragraph': 'text', 'line': 'text'}
        if '<wp:align>' in el:
            a = el.split('<wp:align>')[1].split('<')[0]
            return 'mso-position-%s:%s;mso-position-%s-relative:%s' % (axis, a, axis, mapping[rel])
        off = int(el.split('<wp:posOffset>')[1].split('<')[0])
        return 'margin-%s:%s;mso-position-%s-relative:%s' % ('left' if axis == 'horizontal' else 'top', pt(off), axis, mapping[rel])
    vml = ('<w:pict><v:rect id="Textruta%d" o:spid="_x0000_s%d" style="position:absolute;margin-left:0;margin-top:0;'
           'width:%s;height:%s;z-index:251659264;visibility:visible;mso-wrap-style:square;'
           '%s;%s;mso-wrap-distance-left:9pt;mso-wrap-distance-top:0;mso-wrap-distance-right:9pt;'
           'mso-wrap-distance-bottom:0;v-text-anchor:top" o:gfxdata="" fillcolor="#%s" strokecolor="#%s" strokeweight="1.5pt">'
           '<v:textbox inset="%s,%s,%s,%s"><w:txbxContent>%s</w:txbxContent></v:textbox>'
           '<w10:wrap type="square"/></v:rect></w:pict>'
           % (n, 1026 + n, pt(cx), pt(cy), vml_pos(pos_h, 'horizontal'), vml_pos(pos_v, 'vertical'),
              fill, line, pt(l), pt(tp), pt(r), pt(b), inner)).replace(' o:gfxdata=""', '')
    return (
        '<w:r><w:rPr><w:noProof/></w:rPr><mc:AlternateContent>'
        '<mc:Choice Requires="wps"><w:drawing>'
        '<wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="251659264" '
        'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1" wp14:anchorId="%08X" wp14:editId="%08X">'
        '<wp:simplePos x="0" y="0"/>%s%s<wp:extent cx="%d" cy="%d"/><wp:effectExtent l="0" t="0" r="0" b="0"/>%s'
        '<wp:docPr id="%d" name="%s %d"/><wp:cNvGraphicFramePr/>'
        '<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
        '<a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">'
        '<wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="%d" cy="%d"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="%s"/></a:solidFill>'
        '<a:ln w="%d"><a:solidFill><a:srgbClr val="%s"/></a:solidFill></a:ln></wps:spPr>'
        '<wps:txbx><w:txbxContent>%s</w:txbxContent></wps:txbx>'
        '<wps:bodyPr rot="0" spcFirstLastPara="0" vertOverflow="overflow" horzOverflow="overflow" vert="horz" wrap="square" '
        'lIns="%d" tIns="%d" rIns="%d" bIns="%d" numCol="1" spcCol="0" rtlCol="0" fromWordArt="0" anchor="t" anchorCtr="0" '
        'forceAA="0" compatLnSpc="1"><a:prstTxWarp prst="textNoShape"><a:avLst/></a:prstTxWarp><a:noAutofit/></wps:bodyPr>'
        '</wps:wsp></a:graphicData></a:graphic>'
        '<wp14:sizeRelH relativeFrom="margin"><wp14:pctWidth>0</wp14:pctWidth></wp14:sizeRelH>'
        '<wp14:sizeRelV relativeFrom="margin"><wp14:pctHeight>0</wp14:pctHeight></wp14:sizeRelV>'
        '</wp:anchor></w:drawing></mc:Choice>'
        '<mc:Fallback>%s</mc:Fallback></mc:AlternateContent></w:r>'
        % (0x4E000000 + n, 0x5F000000 + n, pos_h, pos_v, cx, cy, wrap_xml, n + 200, name, n,
           cx, cy, fill, line_w, line, inner, l, tp, r, b, vml))


def pic_anchor(ids, rid, name, cx, cy, pos_h, pos_v, descr=''):
    """Förankrad bild (wp:anchor, fyrkantsomflöde)."""
    n = ids.draw()
    return ('<w:r><w:rPr><w:noProof/></w:rPr><w:drawing>'
            '<wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="251658240" '
            'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1" wp14:anchorId="%08X" wp14:editId="%08X">'
            '<wp:simplePos x="0" y="0"/>%s%s<wp:extent cx="%d" cy="%d"/><wp:effectExtent l="0" t="0" r="0" b="0"/>'
            '<wp:wrapSquare wrapText="bothSides"/><wp:docPr id="%d" name="%s" descr="%s"/>'
            '<wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr>'
            '%s</wp:anchor></w:drawing></w:r>'
            % (0x6A000000 + n, 0x7B000000 + n, pos_h, pos_v, cx, cy, n + 100, escape(name), escape(descr),
               pic_graphic(ids, rid, name, cx, cy, descr)))


def pos_h(rel, align=None, off=0):
    return '<wp:positionH relativeFrom="%s">%s</wp:positionH>' % (rel, '<wp:align>%s</wp:align>' % align if align else '<wp:posOffset>%d</wp:posOffset>' % off)

def pos_v(rel, align=None, off=0):
    return '<wp:positionV relativeFrom="%s">%s</wp:positionV>' % (rel, '<wp:align>%s</wp:align>' % align if align else '<wp:posOffset>%d</wp:posOffset>' % off)


# ---------------------------------------------------------------- delar som är lika i alla dokument

CT_BASE = {
    'rels': 'application/vnd.openxmlformats-package.relationships+xml',
    'xml': 'application/xml',
    'png': 'image/png',
}

def theme_xml():
    colors = [('dk1', '<a:sysClr val="windowText" lastClr="000000"/>'), ('lt1', '<a:sysClr val="window" lastClr="FFFFFF"/>'),
              ('dk2', '<a:srgbClr val="0E2841"/>'), ('lt2', '<a:srgbClr val="E8E8E8"/>'),
              ('accent1', '<a:srgbClr val="156082"/>'), ('accent2', '<a:srgbClr val="E97132"/>'),
              ('accent3', '<a:srgbClr val="196B24"/>'), ('accent4', '<a:srgbClr val="0F9ED5"/>'),
              ('accent5', '<a:srgbClr val="A02B93"/>'), ('accent6', '<a:srgbClr val="4EA72E"/>'),
              ('hlink', '<a:srgbClr val="467886"/>'), ('folHlink', '<a:srgbClr val="96607D"/>')]
    cs = ''.join('<a:%s>%s</a:%s>' % (k, v, k) for k, v in colors)
    fill = '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    ln = lambda w: '<a:ln w="%d" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln>' % w
    return (XML + '<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office-tema">'
            '<a:themeElements><a:clrScheme name="Office">%s</a:clrScheme>'
            '<a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light" panose="020F0302020204030204"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>'
            '<a:minorFont><a:latin typeface="Calibri" panose="020F0502020204030204"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>'
            '<a:fmtScheme name="Office"><a:fillStyleLst>%s%s%s</a:fillStyleLst><a:lnStyleLst>%s%s%s</a:lnStyleLst>'
            '<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>'
            '<a:bgFillStyleLst>%s%s%s</a:bgFillStyleLst></a:fmtScheme></a:themeElements>'
            '<a:objectDefaults/><a:extraClrSchemeLst/></a:theme>' % (cs, fill, fill, fill, ln(6350), ln(12700), ln(19050), fill, fill, fill))


def font_table(fonts):
    out = XML + '<w:fonts xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" xmlns:w16cex="http://schemas.microsoft.com/office/word/2018/wordml/cex" xmlns:w16cid="http://schemas.microsoft.com/office/word/2016/wordml/cid" xmlns:w16="http://schemas.microsoft.com/office/word/2018/wordml" xmlns:w16du="http://schemas.microsoft.com/office/word/2023/wordml/word16du" xmlns:w16sdtdh="http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash" xmlns:w16sdtfl="http://schemas.microsoft.com/office/word/2024/wordml/sdtformatlock" xmlns:w16se="http://schemas.microsoft.com/office/word/2015/wordml/symex" mc:Ignorable="w14 w15 w16se w16cid w16 w16cex w16sdtdh w16sdtfl w16du">'
    for name, panose, family, pitch in fonts:
        out += ('<w:font w:name="%s"><w:panose1 w:val="%s"/><w:charset w:val="00"/><w:family w:val="%s"/><w:pitch w:val="%s"/>'
                '<w:sig w:usb0="E4002EFF" w:usb1="C200247B" w:usb2="00000009" w:usb3="00000000" w:csb0="000001FF" w:csb1="00000000"/></w:font>'
                % (name, panose, family, pitch))
    return out + '</w:fonts>'


FONTS = [('Calibri', '020F0502020204030204', 'swiss', 'variable'), ('Times New Roman', '02020603050405020304', 'roman', 'variable'),
         ('Calibri Light', '020F0302020204030204', 'swiss', 'variable'), ('Aptos', '020B0004020202020204', 'swiss', 'variable')]


def web_settings():
    return (XML + '<w:webSettings xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
            'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
            'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
            'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" '
            'xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" mc:Ignorable="w14 w15">'
            '<w:optimizeForBrowser/><w:allowPNG/></w:webSettings>')


def settings(even_odd, footnote_pr):
    """settings.xml i den ordning schemat kräver: zoom, evenAndOddHeaders, proofState, defaultTabStop,
    characterSpacingControl, footnotePr, endnotePr, compat, rsids ... (evenAndOddHeaders kommer efter
    defaultTabStop i schemat, därför byggs den här separat)."""
    rsids = ''.join('<w:rsid w:val="%s"/>' % r for r in sorted(RSID))
    compat = ''.join('<w:compatSetting w:name="%s" w:uri="http://schemas.microsoft.com/office/word" w:val="%s"/>' % kv for kv in [
        ('compatibilityMode', '15'), ('overrideTableStyleFontSizeAndJustification', '1'), ('enableOpenTypeFeatures', '1'),
        ('doNotFlipMirrorIndents', '1'), ('differentiateMultirowTableHeaders', '1'), ('useWord2013TrackBottomHyphenation', '0')])
    fn = ('<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>'
          '<w:endnotePr><w:endnote w:id="-1"/><w:endnote w:id="0"/></w:endnotePr>') if footnote_pr else ''
    return (XML + '<w:settings %s><w:zoom w:percent="100"/><w:proofState w:spelling="clean" w:grammar="clean"/>'
            '<w:defaultTabStop w:val="720"/>%s<w:hyphenationZone w:val="425"/><w:characterSpacingControl w:val="doNotCompress"/>%s'
            '<w:compat>%s</w:compat><w:rsids><w:rsidRoot w:val="00A1B2C3"/>%s</w:rsids>'
            '<m:mathPr><m:mathFont m:val="Cambria Math"/><m:brkBin m:val="before"/><m:brkBinSub m:val="--"/><m:smallFrac m:val="0"/>'
            '<m:dispDef/><m:lMargin m:val="0"/><m:rMargin m:val="0"/><m:defJc m:val="centerGroup"/><m:wrapIndent m:val="1440"/>'
            '<m:intLim m:val="subSup"/><m:naryLim m:val="undOvr"/></m:mathPr><w:themeFontLang w:val="sv-SE"/>'
            '<w:clrSchemeMapping w:bg1="light1" w:t1="dark1" w:bg2="light2" w:t2="dark2" w:accent1="accent1" w:accent2="accent2" '
            'w:accent3="accent3" w:accent4="accent4" w:accent5="accent5" w:accent6="accent6" w:hyperlink="hyperlink" w:followedHyperlink="followedHyperlink"/>'
            '<w:decimalSymbol w:val=","/><w:listSeparator w:val=";"/></w:settings>'
            % (NS_DECL, '<w:evenAndOddHeaders/>' if even_odd else '', fn, compat, rsids))


def styles_xml(extra_styles=''):
    """styles.xml med docDefaults (Calibri 11 pt, 8 pt efter, 1,08 radavstånd) och Words standardformat."""
    ns = ('xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
          'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
          'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
          'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" '
          'xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" '
          'xmlns:w16cex="http://schemas.microsoft.com/office/word/2018/wordml/cex" '
          'xmlns:w16cid="http://schemas.microsoft.com/office/word/2016/wordml/cid" '
          'xmlns:w16="http://schemas.microsoft.com/office/word/2018/wordml" '
          'xmlns:w16du="http://schemas.microsoft.com/office/word/2023/wordml/word16du" '
          'xmlns:w16sdtdh="http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash" '
          'xmlns:w16sdtfl="http://schemas.microsoft.com/office/word/2024/wordml/sdtformatlock" '
          'xmlns:w16se="http://schemas.microsoft.com/office/word/2015/wordml/symex" '
          'mc:Ignorable="w14 w15 w16se w16cid w16 w16cex w16sdtdh w16sdtfl w16du"')
    lsd = ''.join('<w:lsdException w:name="%s" w:uiPriority="%d" w:qFormat="1"/>' % x for x in
                  [('Normal', 0), ('heading 1', 9), ('heading 2', 9), ('heading 3', 9), ('Title', 10), ('Subtitle', 11)])
    return (XML + '<w:styles %s><w:docDefaults><w:rPrDefault><w:rPr>'
            '<w:rFonts w:asciiTheme="minorHAnsi" w:eastAsiaTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi" w:cstheme="minorBidi"/>'
            '<w:kern w:val="2"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="sv-SE" w:eastAsia="en-US" w:bidi="ar-SA"/>'
            '</w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>'
            '<w:latentStyles w:defLockedState="0" w:defUIPriority="99" w:defSemiHidden="0" w:defUnhideWhenUsed="0" w:defQFormat="0" w:count="376">%s</w:latentStyles>'
            '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>'
            '<w:style w:type="character" w:default="1" w:styleId="Standardstycketeckensnitt"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/><w:unhideWhenUsed/></w:style>'
            '<w:style w:type="table" w:default="1" w:styleId="Normaltabell"><w:name w:val="Normal Table"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>'
            '<w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>'
            '<w:style w:type="numbering" w:default="1" w:styleId="Ingenlista"><w:name w:val="No List"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/></w:style>'
            '<w:style w:type="paragraph" w:styleId="Rubrik1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:link w:val="Rubrik1Char"/><w:uiPriority w:val="9"/><w:qFormat/>'
            '<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="360" w:after="80"/><w:outlineLvl w:val="0"/></w:pPr>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="0F4761" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="40"/><w:szCs w:val="40"/></w:rPr></w:style>'
            '<w:style w:type="paragraph" w:styleId="Rubrik2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:link w:val="Rubrik2Char"/><w:uiPriority w:val="9"/><w:unhideWhenUsed/><w:qFormat/>'
            '<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="160" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="0F4761" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="32"/><w:szCs w:val="32"/></w:rPr></w:style>'
            '<w:style w:type="paragraph" w:styleId="Rubrik3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:link w:val="Rubrik3Char"/><w:uiPriority w:val="9"/><w:unhideWhenUsed/><w:qFormat/>'
            '<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="160" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="0F4761" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>'
            '<w:style w:type="character" w:customStyle="1" w:styleId="Rubrik1Char"><w:name w:val="Rubrik 1 Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Rubrik1"/><w:uiPriority w:val="9"/>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="0F4761" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="40"/><w:szCs w:val="40"/></w:rPr></w:style>'
            '<w:style w:type="character" w:customStyle="1" w:styleId="Rubrik2Char"><w:name w:val="Rubrik 2 Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Rubrik2"/><w:uiPriority w:val="9"/>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="0F4761" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="32"/><w:szCs w:val="32"/></w:rPr></w:style>'
            '<w:style w:type="character" w:customStyle="1" w:styleId="Rubrik3Char"><w:name w:val="Rubrik 3 Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Rubrik3"/><w:uiPriority w:val="9"/>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:color w:val="0F4761" w:themeColor="accent1" w:themeShade="BF"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>'
            '<w:style w:type="paragraph" w:styleId="Rubrik"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:link w:val="RubrikChar"/><w:uiPriority w:val="10"/><w:qFormat/>'
            '<w:pPr><w:spacing w:after="80" w:line="240" w:lineRule="auto"/><w:contextualSpacing/></w:pPr>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:spacing w:val="-10"/><w:kern w:val="28"/><w:sz w:val="56"/><w:szCs w:val="56"/></w:rPr></w:style>'
            '<w:style w:type="character" w:customStyle="1" w:styleId="RubrikChar"><w:name w:val="Rubrik Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Rubrik"/><w:uiPriority w:val="10"/>'
            '<w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:spacing w:val="-10"/><w:kern w:val="28"/><w:sz w:val="56"/><w:szCs w:val="56"/></w:rPr></w:style>'
            '<w:style w:type="paragraph" w:styleId="Underrubrik"><w:name w:val="Subtitle"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:link w:val="UnderrubrikChar"/><w:uiPriority w:val="11"/><w:qFormat/>'
            '<w:numPr><w:ilvl w:val="1"/></w:numPr><w:rPr><w:rFonts w:eastAsiaTheme="majorEastAsia" w:cstheme="majorBidi"/><w:color w:val="595959" w:themeColor="text1" w:themeTint="A6"/><w:spacing w:val="15"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>'
            '<w:style w:type="character" w:customStyle="1" w:styleId="UnderrubrikChar"><w:name w:val="Underrubrik Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Underrubrik"/><w:uiPriority w:val="11"/>'
            '<w:rPr><w:rFonts w:eastAsiaTheme="majorEastAsia" w:cstheme="majorBidi"/><w:color w:val="595959" w:themeColor="text1" w:themeTint="A6"/><w:spacing w:val="15"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>'
            '<w:style w:type="paragraph" w:styleId="Sidhuvud"><w:name w:val="header"/><w:basedOn w:val="Normal"/><w:link w:val="SidhuvudChar"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/>'
            '<w:pPr><w:tabs><w:tab w:val="center" w:pos="4536"/><w:tab w:val="right" w:pos="9072"/></w:tabs><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:style>'
            '<w:style w:type="character" w:customStyle="1" w:styleId="SidhuvudChar"><w:name w:val="Sidhuvud Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Sidhuvud"/><w:uiPriority w:val="99"/></w:style>'
            '<w:style w:type="paragraph" w:styleId="Sidfot"><w:name w:val="footer"/><w:basedOn w:val="Normal"/><w:link w:val="SidfotChar"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/>'
            '<w:pPr><w:tabs><w:tab w:val="center" w:pos="4536"/><w:tab w:val="right" w:pos="9072"/></w:tabs><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:style>'
            '<w:style w:type="character" w:customStyle="1" w:styleId="SidfotChar"><w:name w:val="Sidfot Char"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:link w:val="Sidfot"/><w:uiPriority w:val="99"/></w:style>'
            '<w:style w:type="character" w:styleId="Hyperlnk"><w:name w:val="Hyperlink"/><w:basedOn w:val="Standardstycketeckensnitt"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:color w:val="467886" w:themeColor="hyperlink"/><w:u w:val="single"/></w:rPr></w:style>'
            '<w:style w:type="table" w:styleId="Tabellrutnt"><w:name w:val="Table Grid"/><w:basedOn w:val="Normaltabell"/><w:uiPriority w:val="39"/>'
            '<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders>'
            + ''.join('<w:%s w:val="single" w:sz="4" w:space="0" w:color="auto"/>' % s for s in ['top', 'left', 'bottom', 'right', 'insideH', 'insideV'])
            + '</w:tblBorders></w:tblPr></w:style>'
            + extra_styles + '</w:styles>') % (ns, lsd)


def app_xml(pages, words, chars, title_parts=1):
    return (XML + '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" '
            'xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">'
            '<Template>Normal.dotm</Template><TotalTime>0</TotalTime><Pages>%d</Pages><Words>%d</Words>'
            '<Characters>%d</Characters><Application>Microsoft Office Word</Application><DocSecurity>0</DocSecurity>'
            '<Lines>1</Lines><Paragraphs>1</Paragraphs><ScaleCrop>false</ScaleCrop><Company></Company>'
            '<LinksUpToDate>false</LinksUpToDate><CharactersWithSpaces>%d</CharactersWithSpaces><SharedDoc>false</SharedDoc>'
            '<HyperlinksChanged>false</HyperlinksChanged><AppVersion>16.0000</AppVersion></Properties>'
            % (pages, words, chars, chars))


def core_xml(title, creator):
    return (XML + '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
            'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
            'xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
            '<dc:title>%s</dc:title><dc:creator>%s</dc:creator><cp:lastModifiedBy>%s</cp:lastModifiedBy><cp:revision>3</cp:revision>'
            '<dcterms:created xsi:type="dcterms:W3CDTF">2026-03-02T08:15:00Z</dcterms:created>'
            '<dcterms:modified xsi:type="dcterms:W3CDTF">2026-03-02T09:40:00Z</dcterms:modified></cp:coreProperties>'
            % (escape(title), escape(creator), escape(creator)))


REL_BASE = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/'


def rels(items):
    """items: [(id, typ-suffix eller full, target)] -> Relationships-XML."""
    out = XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    for rid, typ, target in items:
        typ = typ if typ.startswith('http') else REL_BASE + typ
        out += '<Relationship Id="%s" Type="%s" Target="%s"%s/>' % (rid, typ, target, ' TargetMode="External"' if target.startswith('http') else '')
    return out + '</Relationships>'


def write_docx(path, parts, content_types):
    """parts: {namn: bytes eller str}. [Content_Types].xml och _rels/.rels läggs först, som i Word.
    Fast tidsstämpel så att filen blir densamma varje körning."""
    ct = XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    ct += '<Default Extension="rels" ContentType="%s"/><Default Extension="xml" ContentType="application/xml"/>' % CT_BASE['rels']
    if any(n.endswith('.png') for n in parts):
        ct += '<Default Extension="png" ContentType="image/png"/>'
    for name, typ in content_types:
        ct += '<Override PartName="/%s" ContentType="%s"/>' % (name, typ)
    ct += '</Types>'
    order = ['[Content_Types].xml', '_rels/.rels'] + [n for n in parts if n not in ('[Content_Types].xml', '_rels/.rels')]
    allp = dict(parts)
    allp['[Content_Types].xml'] = ct
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
        for n in order:
            zi = zipfile.ZipInfo(n, (2026, 3, 2, 9, 40, 0))
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = 0o600 << 16
            d = allp[n]
            z.writestr(zi, d if isinstance(d, bytes) else d.encode('utf-8'))


WML = 'application/vnd.openxmlformats-officedocument.wordprocessingml.'
CT = {
    'document': WML + 'document.main+xml', 'styles': WML + 'styles+xml', 'settings': WML + 'settings+xml',
    'webSettings': WML + 'webSettings+xml', 'fontTable': WML + 'fontTable+xml', 'numbering': WML + 'numbering+xml',
    'footnotes': WML + 'footnotes+xml', 'endnotes': WML + 'endnotes+xml', 'header': WML + 'header+xml',
    'footer': WML + 'footer+xml', 'theme': 'application/vnd.openxmlformats-officedocument.theme+xml',
    'core': 'application/vnd.openxmlformats-package.core-properties+xml',
    'app': 'application/vnd.openxmlformats-officedocument.extended-properties+xml',
}


def root_rels():
    return rels([('rId3', 'extended-properties'.join(['http://schemas.openxmlformats.org/officeDocument/2006/relationships/', '']), 'docProps/app.xml'),
                 ('rId2', 'http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties', 'docProps/core.xml'),
                 ('rId1', 'officeDocument', 'word/document.xml')])
