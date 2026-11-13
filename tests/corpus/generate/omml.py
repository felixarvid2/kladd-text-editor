import zipfile, re, sys
src, dst = sys.argv[1], sys.argv[2]
zin = zipfile.ZipFile(src)
omml = ('<m:oMath><m:sSup><m:e><m:d><m:e><m:r><m:t>a+b</m:t></m:r></m:e></m:d></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>'
        '<m:r><m:t>=</m:t></m:r><m:sSup><m:e><m:r><m:t>a</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>'
        '<m:r><m:t>+2ab+</m:t></m:r><m:sSup><m:e><m:r><m:t>b</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath>')
with zipfile.ZipFile(dst, 'w', zipfile.ZIP_DEFLATED) as zout:
    for item in zin.infolist():
        data = zin.read(item.filename)
        if item.filename == 'word/document.xml':
            s = data.decode('utf-8')
            s = s.replace('<w:document ', '<w:document xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" ', 1)
            s, n = re.subn(r'<w:r><w:rPr></w:rPr><w:drawing>.*?</w:drawing></w:r>', lambda m: omml, s, count=1, flags=re.S)
            assert n == 1
            data = s.encode('utf-8')
        zout.writestr(item.filename, data)
