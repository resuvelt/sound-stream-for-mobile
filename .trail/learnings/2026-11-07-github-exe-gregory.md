---
title: "Релиз выпускается тегом: сборщик GitHub сам собирает exe и прикладывает его к релизу"
author: Gregory
date: 2026-11-07
branch: main
cost: { minutes: 4, tokens: 42443 }
id: l-957346f0
scope: repo
status: active
src: agent
touches:
  - ".github/workflows/release.yml"
  - README.md
  - ".cargo/config.toml"
  - Cargo.toml
  - src/main.rs
  - src/wide.rs
  - src/log.rs
  - src/assets.rs
  - src/state.rs
  - public/icon-512.png
---

## Decided
Готовый exe скачивают из GitHub Releases, собирать его никому не нужно. Релиз выпускается пушем аннотированного тега с номером версии без «v» (как 1.0, 1.1, 1.2, 2.0); GitHub Actions собирает exe на windows-latest, проверяет, что он без консоли и без runtime-DLL, и создаёт релиз с файлом sound-stream.exe. Текст релиза берётся из сообщения тега, пишем его по-английски. В README — без ссылок на скачивание, только «скачай из Releases».

## Tried and ruled out
Выкладывать exe с локальной машины через gh: здесь gh не авторизован, а сборка в CI ещё и гарантирует, что exe собран из закоммиченного кода.

## Watch out
Сообщение тега с заголовками markdown («## What's new») создавай с --cleanup=verbatim, иначе git при редактировании выкинет строки на «#» как комментарии.
Если релиз для тега уже есть, сборка не пересоздаёт его, а перезаливает exe (--clobber).
Версию в Cargo.toml поднимай вместе с тегом.

In the code: .github/workflows/release.yml (триггер tags: '[0-9]*', permissions contents: write). Выпуск: git tag -a 2.1 --cleanup=verbatim -F notes.md && git push origin 2.1.
