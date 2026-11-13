# Kodsignering och paketering

Releasen (`.github/workflows/release.yml`) kan signera Windows- och macOS-paketen, men bara om
hemligheterna nedan finns i repot. Utan dem bygger allt precis som förut: Windows osignerat och
macOS med ad hoc-signering. Inget steg misslyckas för att en hemlighet saknas.

Hemligheter läggs in under *Settings → Secrets and variables → Actions → New repository secret*.
Stegen i workflowen har villkor av typen `if: env.X != ''`, eftersom `secrets` inte går att använda
direkt i `if:`.

## Windows

Utan signering varnar SmartScreen ("Windows skyddade din dator"). Med signering försvinner varningen
först när certifikatet har fått rykte: ett OV-certifikat bygger upp det över tid, ett EV-certifikat
eller Azure Trusted Signing ger det i stort sett direkt.

### Vad man skaffar

- **Azure Trusted Signing** (billigast, ca 10 USD i månaden, kräver en verifierad identitet och för
  närvarande organisation eller verifierad privatperson i vissa länder), eller
- **OV-kodsigneringscertifikat** (ca 200–400 USD per år) från till exempel Sectigo, DigiCert eller
  SSL.com, eller
- **EV-certifikat** (dyrare, ligger oftast på ett hårdvarutoken eller i molnet och går då inte att
  exportera som `.pfx`, så det passar inte stegen här).

Sedan 2023 måste nyutfärdade certifikat ligga på hårdvara eller i en molntjänst. Många leverantörer
säljer därför bara certifikat som inte går att exportera. Kontrollera att leverantören ger en
exporterbar `.pfx`, annars använd Azure Trusted Signing.

### Hemligheter (PFX, det som workflowen använder idag)

| Hemlighet | Innehåll |
|---|---|
| `WINDOWS_CERT_PFX_BASE64` | `.pfx`-filen, base64-kodad |
| `WINDOWS_CERT_PASSWORD` | lösenordet till `.pfx`-filen |

Kodning (en rad, utan radbrytningar):

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("kladd.pfx")) | Set-Clipboard
```

```bash
base64 -w0 kladd.pfx   # Linux; på macOS: base64 -i kladd.pfx
```

Workflowen signerar `kladd.exe` (före zip och installationsprogram, så att båda innehåller den
signerade filen) och därefter `Kladd-<version>-windows-x64-setup.exe`, med SHA-256 och tidsstämpel
från `http://timestamp.digicert.com`. Koden ligger i `packaging/windows/sign.ps1`. Varje fil
verifieras direkt med `signtool verify /pa`.

### Alternativ: Azure Trusted Signing (inte inkopplat)

Det här är inte inbyggt i workflowen. Vill man använda det byter man ut stegen *Signera kladd.exe*
och *Signera installationsprogrammet* mot `azure/trusted-signing-action` med hemligheterna
`AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` samt kontots endpoint, kontonamn och
certifikatprofil. Åtgärden signerar filer med ett filmönster, så den körs en gång på
`target\release\kladd.exe` och en gång på `dist\*-setup.exe`.

## macOS

Utan Developer ID-signering och notarisering måste användaren högerklicka och välja *Öppna*.

### Vad man skaffar

1. **Apple Developer Program** (99 USD per år), https://developer.apple.com/programs/.
2. I *Certificates, Identifiers & Profiles* skapar du ett certifikat av typen
   **Developer ID Application** (ladda upp en CSR från Nyckelhanteraren).
3. Ett **applikationsspecifikt lösenord** på https://account.apple.com (*Logga in och säkerhet →
   Applikationsspecifika lösenord*) för notariseringen.
4. **Team-ID**: tio tecken, står under *Membership details*.

### Exportera certifikatet

I Nyckelhanteraren: markera certifikatet *Developer ID Application: ...* tillsammans med dess
privata nyckel, välj *Exportera* och spara som `.p12` med ett lösenord. Kodning:

```bash
base64 -i kladd.p12 | pbcopy
```

Namnet att använda som signeringsidentitet visas av
`security find-identity -v -p codesigning` (till exempel `Developer ID Application: Namn (TEAMID)`).

### Hemligheter

| Hemlighet | Innehåll |
|---|---|
| `MACOS_CERT_P12_BASE64` | `.p12`-filen, base64-kodad |
| `MACOS_CERT_PASSWORD` | lösenordet till `.p12`-filen |
| `MACOS_SIGN_IDENTITY` | hela identitetsnamnet, t.ex. `Developer ID Application: Namn (TEAMID)` |
| `APPLE_ID` | Apple-kontots e-postadress (bara för notarisering) |
| `APPLE_TEAM_ID` | Team-ID (bara för notarisering) |
| `APPLE_APP_PASSWORD` | det applikationsspecifika lösenordet (bara för notarisering) |

Med de tre första hemligheterna importeras certifikatet till en tillfällig nyckelring, appen
signeras med `codesign --options runtime --timestamp` och rättigheterna i
`packaging/macos/kladd.entitlements` (medvetet tom: lägg bara till nycklar som behövs), och
`.dmg`:n signeras. Med alla sex skickas `.dmg`:n också till `xcrun notarytool submit --wait`
och kvittot fästs med `xcrun stapler staple`. Nyckelringen tas bort sist i jobbet.

## Provköra

- Windows: kör workflowen för hand (*Actions → Release → Run workflow*, utan att kryssa i
  publicera). Ladda ner artefakten `kladd-windows` och kontrollera egenskaperna på `kladd.exe`
  och setup-filen (fliken *Digitala signaturer*), eller `signtool verify /pa /v fil.exe`.
- macOS: ladda ner artefakten `kladd-macos`, montera `.dmg`:n och kör
  `codesign -dvv --deep Kladd.app`, `spctl -a -vv Kladd.app` (ska säga *Notarized Developer ID*)
  och `xcrun stapler validate Kladd-*.dmg`.
- Börja gärna med en egen testgren och bara en plattform i taget: sätt hemligheterna, kör
  workflowen och läs jobbloggen. Ett fel i signeringen stoppar jobbet.

## Linux

- **.deb**: osignerat. Det finns inget motsvarande installationsvarsel att ta bort.
- **AppImage**: byggs i jobbet `appimage` med `packaging/linux/build-appimage.sh` (linuxdeploy,
  `APPIMAGE_EXTRACT_AND_RUN=1` eftersom FUSE saknas på körmiljön). Grafik- och fönsterbibliotek
  (libGL, libEGL, libwayland, libX*, libxkbcommon) tas inte med; de används från värdsystemet.
  Det går att köra lokalt: `packaging/linux/build-appimage.sh <version> [binär]`. Filen är
  osignerad (AppImage har valfri GPG-signering som vi inte använder).
- **Flatpak**: manifestet `packaging/flatpak/se.kladd.Kladd.yml` och AppStream-filen
  `se.kladd.Kladd.metainfo.xml` är förberedda för en framtida Flathub-inlämning men byggs inte i
  CI och är inte provbyggda. Före inlämning: generera `cargo-sources.json` med
  `flatpak-cargo-generator.py Cargo.lock` och ta bort `--share=network`, peka källan på en taggad
  version, lägg till skärmbilder i metainfo och kör `appstreamcli validate`.
