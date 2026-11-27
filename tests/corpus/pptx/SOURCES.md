# Presentationer i korpusen

Filerna `pythonpptx-*.pptx` är orörda kopior av testfiler ur python-pptx
(https://github.com/scanny/python-pptx, commit 278b47b1dedd5b46ee84c286e77cdfb0bf4594be,
`features/steps/test_files/`), skapade av python-pptx författare i PowerPoint för Mac 2011. Licens: MIT.

    The MIT License (MIT)
    Copyright (c) 2013 Steve Canny, https://github.com/scanny

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in
    all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
    THE SOFTWARE.

Varje fil har PowerPoints egen miniatyr av första bilden (`docProps/thumbnail.jpeg`). Provet
`pptx::tests::corpus_pptx_miniatyrer` renderar bild 1 i miniatyrens storlek och jämför (samma mått som
jämförelseprovet, se `docs/dev/pptx.md`); värdet i `manifest.toml` får inte sjunka mer än 0,03.
Filerna använder Calibri, som Kladd ersätter med det inbyggda Carlito, så måttet beror inte på
vilka typsnitt som finns installerade (utom för enstaka symboler).

Skriv om värdena med `KLADD_CORPUS_BLESS=1 cargo test corpus_pptx -- --nocapture` (provet skriver ut
den nya manifesttexten) och granska ändringen.
