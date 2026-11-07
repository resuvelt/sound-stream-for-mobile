---
title: Exe должен запускаться на чистой Windows без Visual C++ Redistributable
author: Gregory
date: 2026-11-07
branch: main
cost: { minutes: 3, tokens: 21032 }
id: l-cee8fb11
scope: repo
status: active
src: agent
touches:
  - ".cargo/config.toml"
  - README.md
  - Cargo.lock
  - Cargo.toml
  - assets/sound-stream.ico
  - build.rs
---

## Decided
Сборка линкует C-рантайм статически (+crt-static), чтобы exe можно было просто скопировать на другой ПК с Windows 10/11 x64 и запустить без установки чего-либо. Импорт exe — только системные DLL Windows.

## Tried and ruled out
Обычная MSVC-сборка Rust тянет VCRUNTIME140.dll (и api-ms-win-crt-*): на ПК без Visual C++ Redistributable exe не стартует с ошибкой об отсутствующей DLL. Ставить Redistributable пользователю — не вариант.

## Watch out
После смены зависимостей или флагов сборки проверь импорт exe: в нём не должно быть vcruntime*/msvcp*/api-ms-win-crt-*.
Пока sound-stream.exe запущен (в т.ч. из автозапуска), cargo build --release не может перезаписать файл — сначала закрой программу (меню трея → «Выход»), после сборки запусти снова.

In the code: .cargo/config.toml — [target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-feature=+crt-static"]; exe — target\release\sound-stream.exe.
