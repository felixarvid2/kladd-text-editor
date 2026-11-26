# Prestandaprov i CI

Kladds tyngsta vägar mäts i **instruktioner** (valgrind `--tool=callgrind`), inte i tid. Instruktionsantalet är
detsamma körning efter körning (mätt: under 1 % spridning, oftast under 0,1 %) oavsett hur belastad en delad CI-maskin är,
så en gräns på 10 % ger inga falsklarm av brus. Arbetsflödet `.github/workflows/perf.yml` bygger och mäter både PR:ens
kod och basgrenens i samma jobb (samma maskin, samma kompilator, samma valgrind) och jämför dem; det finns ingen
incheckad baslinje som kan bli inaktuell när kompilatorn eller Ubuntu-avbildningen uppdateras.

| Prov (`src/perfci.rs`) | Vad |
|---|---|
| `perfci_docx_read_mt` | läsa ett stort .docx (zip/XML till Markdown) |
| `perfci_odt_read_mt` | läsa ett stort .odt |
| `perfci_docx_open` | analys och kall utläggning med sidindelning av ett ~100-sidigt dokument |
| `perfci_docx_keys` | ett tecken på fem ställen i dokumentet (inkrementell analys och utläggning) |
| `perfci_docx_search` | tangenttryck med sökningen öppen (sökning, träfflager, utläggning) |
| `perfci_md_read` | läsläget för 400 kB Markdown: stilla, rullning och mitt i filen |
| `perfci_pdf_export_mt` | PDF-export av ett medelstort dokument (hela processen räknas, formningen går i trådar) |

Dokumenten genereras av Kladds egna skrivare med fasta frön (`perfci_fixtures`), så inget hämtas från nätet. Varje prov slår
på räkningen bara kring själva mätningen (en callgrind-förfrågan i `Counting`), så att uppstart och uppvärmning inte räknas.
Hela jobbet tar ungefär en minut att mäta per version; byggen är det som tar tid (cachas av `rust-cache`).

## Köra lokalt

Kräver `valgrind` och Python 3.

```
perf/run.sh ut/pr                  # mäter trädet du står i  -> ut/pr/ir.json
git worktree add ../bas origin/main
perf/overlay.sh ../bas             # samma prov i basen (om den saknar dem)
perf/run.sh ut/bas ../bas
perf/compare.py ut/bas/ir.json ut/pr/ir.json --fail 10 --warn 3
```

`PERF_TARGET_DIR` byter målkatalog (standard `target-perf`), `PERF_JOBS` antal samtidiga prov. Tabellen visar bas, PR och
ändring per prov; utgångskoden är 1 om något prov ökat mer än `--fail` procent. Bara siffror från samma maskin och
verktygsversion går att jämföra. Skriptet har egna prov: `python3 perf/test_compare.py`.

## Köra i CI

Arbetsflödet körs på varje pull request (utom när bara dokumentation ändrats) och för hand (Actions, Prestanda, Run workflow)
med valfri `base_ref` (gren, tagg eller commit), `fail` och `warn`. Tabellen står i jobbets sammanfattning, fel och varningar
som annotationer. Går basen inte att bygga med PR:ens prov (de inre gränssnitt proven använder har ändrats) visas bara PR:ens
siffror och en varning, jobbet stoppas inte.

## Lägga till eller ändra ett prov

1. Skriv ett `#[test] #[ignore] fn perfci_<namn>()` i `src/perfci.rs`. Lägg bara det som ska mätas i en `#[inline(never)]`
   funktion som börjar med `let _count = Counting::on();`; byggs allt upp före den (utläggning, öppning) räknas det inte.
   Prov som använder trådar (callgrind-räkningen gäller bara den tråd som slår på den) får namn som slutar på `_mt`: de använder inte `Counting` och hela processen räknas.
2. Håll provet under någon sekund naturligt (valgrind är ~30–50 gånger långsammare).
3. `perf/run.sh ut/a . && perf/run.sh ut/b .` ska ge (nästan) samma tal; stora skillnader betyder att provet inte är deterministiskt
   (klocktid, trådar, slump) och bör ändras.

## Ändra en medvetet långsammare väg

Ökar ett prov över gränsen av en bra anledning (mer arbete för ett nytt krav): skriv varför i PR:ens beskrivning. Jobbet är
rött men inget hindrar sammanslagning; mät då gärna riktig tid också (`docs/dev/prestanda.md`). Ändras själva proven jämförs
PR:ens prov mot basens kod, så en provändring ger aldrig en falsk ökning.
