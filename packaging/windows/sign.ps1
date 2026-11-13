# Hjälpfunktioner för Authenticode-signering i release-workflowen. Används bara när
# hemligheterna WINDOWS_CERT_PFX_BASE64 och WINDOWS_CERT_PASSWORD finns.
#   . packaging\windows\sign.ps1; Initialize-Signing; Sign-File target\release\kladd.exe

function Initialize-Signing {
    $script:Pfx = Join-Path $env:RUNNER_TEMP "kladd-cert.pfx"
    [IO.File]::WriteAllBytes($script:Pfx, [Convert]::FromBase64String($env:WINDOWS_CERT_PFX_BASE64))
    # signtool följer med Windows SDK på körmiljön; ta den senaste x64-versionen.
    $script:SignTool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending | Select-Object -First 1 -ExpandProperty FullName
    if (-not $script:SignTool) { throw "Hittade inte signtool.exe" }
}

function Sign-File([string]$Path) {
    & $script:SignTool sign /fd sha256 /f $script:Pfx /p $env:WINDOWS_CERT_PASSWORD `
        /tr http://timestamp.digicert.com /td sha256 /d "Kladd" $Path
    if ($LASTEXITCODE -ne 0) { throw "signtool sign misslyckades för $Path" }
    & $script:SignTool verify /pa $Path
    if ($LASTEXITCODE -ne 0) { throw "signtool verify misslyckades för $Path" }
}
