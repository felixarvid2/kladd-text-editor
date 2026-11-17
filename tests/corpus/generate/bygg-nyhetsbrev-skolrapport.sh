#!/bin/sh
# Bygger de sex nya gen-dokumenten (nyhetsbrev och skolrapport) i Word- och LibreOffice-stil.
# Körs från generate/: sh bygg-nyhetsbrev-skolrapport.sh
set -e
cd "$(dirname "$0")"
T=$(mktemp -d)
python3 mknyhetsbrev.py ../docx
python3 mkskolrapport.py ../docx
python3 mklo.py .
for n in nyhetsbrev skolrapport; do
  soffice -env:UserInstallation=file://$T/p --headless --convert-to odt  --outdir "$T" $n-lo.fodt >/dev/null
  soffice -env:UserInstallation=file://$T/p --headless --convert-to docx --outdir "$T" $n-lo.fodt >/dev/null
  cp "$T/$n-lo.odt"  ../odt/gen-$n-lo.odt
  cp "$T/$n-lo.docx" ../docx/gen-$n-lo.docx
done
rm -rf "$T"
