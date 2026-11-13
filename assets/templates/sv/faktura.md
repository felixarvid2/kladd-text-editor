<!-- mall; kategori: faktura; läge: dokument; namn: Faktura; beskrivning: Faktura med avsändare, kund, tabell över rader och betalningsvillkor. -->
<!-- kladd: page-size: A4; page-margin: 20mm 20mm 20mm 20mm; footer: "{{företag}}|{{orgnr}}|Sida {sida} av {sidor}"; h1: 28pt bold #1F3864 space-before 0pt space-after 6pt; h2: 12pt bold #2F5496 space-before 12pt space-after 3pt; body: space-after 4pt -->

# FAKTURA

| **Fakturanummer** | {{fakturanummer}} |
| --- | --- |
| **Fakturadatum** | {{datum}} |
| **Förfallodatum** | {{förfallodatum}} |

## Från

{{företag}}
{{adress}}, {{postnummer}} {{ort}}
Organisationsnummer: {{orgnr}}

## Till

{{kund}}
{{kundens-adress}}

## Specifikation

| Beskrivning | Antal | À-pris | Belopp |
| --- | ---: | ---: | ---: |
| Tjänst eller vara | 1 | 0,00 | 0,00 |
| Rad två | | | |
| Rad tre | | | |
| **Summa exkl. moms** | | | **0,00** |
| Moms 25 % | | | 0,00 |
| **Att betala** | | | **0,00** |

## Betalning

Betala senast {{förfallodatum}} till bankgiro {{bankgiro}}. Ange fakturanummer {{fakturanummer}} som referens.
