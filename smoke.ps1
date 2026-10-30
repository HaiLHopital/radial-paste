$ErrorActionPreference = 'Stop'
$env:RUST_LOG = 'warn,radial=debug,wgpu=off,wgpu_hal=off'
$exe = Join-Path $PSScriptRoot 'target\debug\radial-paste.exe'
$err = Join-Path $PSScriptRoot 'smoke-err.log'
Remove-Item $err -ErrorAction SilentlyContinue

Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
'@

# A known value, so the restore can be checked.
Set-Clipboard -Value 'ORIGINAL-CONTENT'

$app = Start-Process -FilePath $exe -PassThru -RedirectStandardError $err -WindowStyle Hidden
Start-Sleep -Milliseconds 2000

# Notepad gets the focus and receives the paste.
Start-Process -FilePath 'notepad.exe' | Out-Null
Start-Sleep -Milliseconds 3000

# Hold the hotkey and flick right, then let go. The combo below is Ctrl+Shift+Space: this
# script does not read your config.toml, so a different hotkey there means nothing happens.
[W.U]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
[W.U]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)
[W.U]::keybd_event(0x20, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 150
[W.U]::mouse_event(0x0001, 300, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60
[W.U]::keybd_event(0x20, 0, 2, [UIntPtr]::Zero)
[W.U]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero)
[W.U]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 1200

Stop-Process -Id $app.Id -Force
# This line can be misleading: a clipboard manager rewrites the clipboard after the paste, so it
# may print ORIGINAL-CONTENT even though the restore was skipped by design. Check Notepad.
"clipboard after: $(Get-Clipboard)"
Get-Content $err
"Notepad was left open; it should contain a snippet."
