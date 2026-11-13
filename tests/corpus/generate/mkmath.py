import zipfile, re, sys
src = open('innehall-formler.fodt', encoding='utf-8').read()
m = re.search(r'<draw:object>(<math:math>.*</math:math>)</draw:object>', src, re.S)
math = m.group(1)
src = src.replace(m.group(0), '<draw:object xlink:href="./Object 1" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>')
# Delar upp det platta dokumentet i content/styles/meta för ett riktigt paket.
def part(tag):
    return re.search(r'(<office:%s>.*</office:%s>|<office:%s/>)' % (tag, tag, tag), src, re.S).group(1)
ns = re.search(r'<office:document ([^>]*?) office:version', src).group(1)
def wrap(root, inner):
    return '<?xml version="1.0" encoding="UTF-8"?>\n<office:%s %s office:version="1.3">%s</office:%s>' % (root, ns, inner, root)
content = wrap('document-content', part('font-face-decls') + part('automatic-styles') + part('body'))
styles = wrap('document-styles', part('styles') + part('master-styles'))
meta = wrap('document-meta', part('meta'))
mathdoc = '<?xml version="1.0" encoding="UTF-8"?>\n' + math.replace('<math:math>', '<math:math xmlns:math="http://www.w3.org/1998/Math/MathML">', 1)
manifest = '''<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
<manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.text"/>
<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
<manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/>
<manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>
<manifest:file-entry manifest:full-path="Object 1/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.formula"/>
<manifest:file-entry manifest:full-path="Object 1/content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>'''
with zipfile.ZipFile(sys.argv[1], 'w') as z:
    z.writestr(zipfile.ZipInfo('mimetype'), 'application/vnd.oasis.opendocument.text')
    z.writestr('content.xml', content, zipfile.ZIP_DEFLATED)
    z.writestr('styles.xml', styles, zipfile.ZIP_DEFLATED)
    z.writestr('meta.xml', meta, zipfile.ZIP_DEFLATED)
    z.writestr('Object 1/content.xml', mathdoc, zipfile.ZIP_DEFLATED)
    z.writestr('META-INF/manifest.xml', manifest, zipfile.ZIP_DEFLATED)
