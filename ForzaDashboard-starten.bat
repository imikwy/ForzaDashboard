@echo off
chcp 65001 >nul
cd /d "%~dp0"
title ForzaDashboard
where node >nul 2>&1
if errorlevel 1 (
  echo.
  echo   Node.js ist noch nicht installiert.
  echo.
  echo   1. Gleich oeffnet sich nodejs.org - dort die LTS-Version herunterladen
  echo      und ganz normal installieren ^(alle Haken so lassen^).
  echo   2. Danach diese Datei nochmal doppelklicken.
  echo.
  start "" "https://nodejs.org/de/download"
  pause
  exit /b 1
)
node "%~dp0starten.js"
echo.
pause
