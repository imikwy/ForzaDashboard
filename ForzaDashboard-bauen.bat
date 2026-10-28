@echo off
chcp 65001 >nul
cd /d "%~dp0"
title ForzaDashboard bauen
REM Baut ForzaDashboard.exe (Tauri + Rust) aus app\ und src\dashboard.html.
REM BAU_VER in app\src\main.rs wird automatisch erhöht (JJJJ-MM-TT.N). Für alle Spieler: Programm-Release auf GitHub (siehe README.md).
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>&1
if errorlevel 1 (
  echo Rust ist nicht installiert: https://rustup.rs
  pause
  exit /b 1
)
REM Version erhöhen: heute schon gebaut -> N+1, sonst heute.1
powershell -NoProfile -Command "$f='app\src\main.rs'; $t=[IO.File]::ReadAllText($f); $d=Get-Date -Format 'yyyy-MM-dd'; $m=[regex]::Match($t,'BAU_VER: &str = \"([0-9-]+)\.(\d+)\"'); $n= if ($m.Groups[1].Value -eq $d) { [int]$m.Groups[2].Value + 1 } else { 1 }; $v=\"$d.$n\"; [IO.File]::WriteAllText($f, $t.Replace($m.Value, 'BAU_VER: &str = \"' + $v + '\"')); Write-Host \"Version $v\""
taskkill /im ForzaDashboard.exe /f >nul 2>&1
echo Baue ForzaDashboard.exe ... ^(das erste Mal dauert einige Minuten^)
cargo build --release --manifest-path app\Cargo.toml > build-log.txt 2>&1
if errorlevel 1 (
  type build-log.txt
  echo.
  echo FEHLER beim Bauen - bitte die Datei build-log.txt an Claude schicken.
  pause
  exit /b 1
)
copy /y app\target\release\ForzaDashboard.exe ForzaDashboard.exe >nul
REM Test-Oberflächen im Datenordner entfernen, sonst überdecken sie die eingebaute neue Version
del /q "%APPDATA%\ForzaDashboard\dashboard.html" >nul 2>&1
del /q "%~dp0dashboard.html" >nul 2>&1
REM Installation auf dem Desktop (falls vorhanden) gleich mit aktualisieren und von dort starten
set "ZIEL=%USERPROFILE%\Desktop\ForzaDashboard"
set "START=%~dp0ForzaDashboard.exe"
if exist "%ZIEL%\ForzaDashboard.exe" (
  copy /y app\target\release\ForzaDashboard.exe "%ZIEL%\ForzaDashboard.exe" >nul
  REM dort weiß die exe so, wo die Repo-Kopie liegt (für "Veröffentlichen" im Dashboard)
  if exist "%ZIEL%\einstellungen.txt" findstr /b /c:"repo_ordner=" "%ZIEL%\einstellungen.txt" >nul || echo repo_ordner=%~dp0>>"%ZIEL%\einstellungen.txt"
  del /q "%ZIEL%\dashboard.html" >nul 2>&1
  set "START=%ZIEL%\ForzaDashboard.exe"
)
echo.
echo Fertig! ForzaDashboard.exe wurde erstellt und wird jetzt gestartet.
start "" "%START%"
timeout /t 4 >nul 2>&1
exit /b 0
