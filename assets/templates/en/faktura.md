<!-- mall; kategori: faktura; läge: dokument; namn: Invoice; beskrivning: Invoice with sender, customer, a table of line items and payment terms. -->
<!-- kladd: page-size: Letter; page-margin: 0.8in 0.8in 0.8in 0.8in; footer: "{{company}}|{{tax-id}}|Page {sida} of {sidor}"; h1: 28pt bold #1F3864 space-before 0pt space-after 6pt; h2: 12pt bold #2F5496 space-before 12pt space-after 3pt; body: space-after 4pt -->

# INVOICE

| **Invoice number** | {{invoice-number}} |
| --- | --- |
| **Invoice date** | {{date}} |
| **Due date** | {{due-date}} |

## From

{{company}}
{{address}}, {{city}} {{zip}}
Tax ID: {{tax-id}}

## Bill to

{{customer}}
{{customer-address}}

## Details

| Description | Qty | Unit price | Amount |
| --- | ---: | ---: | ---: |
| Service or item | 1 | 0.00 | 0.00 |
| Line two | | | |
| Line three | | | |
| **Subtotal** | | | **0.00** |
| Tax 25% | | | 0.00 |
| **Amount due** | | | **0.00** |

## Payment

Please pay by {{due-date}} to account {{account}}. Quote invoice number {{invoice-number}} as the reference.
