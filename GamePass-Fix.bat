@echo off
REM Nur noetig, wenn Forza ueber Game Pass / Microsoft Store laeuft
REM und das Dashboard trotz richtiger Einstellungen keine Daten bekommt.
net session >nul 2>&1
if errorlevel 1 (
  powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
  exit /b
)
powershell -NoProfile -Command "$p = Get-AppxPackage *Forza*; if (-not $p) { Write-Host 'Keine Store-Version von Forza gefunden (Steam braucht diesen Fix nicht).' } else { $p | ForEach-Object { CheckNetIsolation LoopbackExempt -a -n=$($_.PackageFamilyName) | Out-Null; Write-Host ('Freigegeben: ' + $_.Name) } }"
echo.
pause
