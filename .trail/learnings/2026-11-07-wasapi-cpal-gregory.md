---
title: "Системный звук захватываем напрямую через WASAPI, а не через cpal"
author: Gregory
date: 2026-11-07
branch: main
cost: { minutes: 24, tokens: 545667 }
id: l-92859675
scope: repo
status: active
src: agent
touches:
  - Cargo.toml
  - src/main.rs
  - src/wide.rs
  - src/log.rs
  - src/assets.rs
  - src/state.rs
  - src/capture.rs
  - src/server.rs
  - src/net.rs
  - src/mdns.rs
---

## Decided
Захват loopback написан на «голом» WASAPI (крейт windows), без cpal. Устройство вывода по умолчанию открывается на вход с флагами LOOPBACK | EVENTCALLBACK, раз в секунду сверяется ID устройства по умолчанию; при смене захват переоткрывается и слушателям уходит новый format.

## Tried and ruled out
cpal 0.15 (loopback = input-поток на output-устройстве): игнорирует AUDCLNT_BUFFERFLAGS_SILENT (по документации содержимое такого буфера надо считать тишиной, иначе возможен мусор) и даёт только имя устройства, а не ID — смену устройства по умолчанию надёжно не отследить. Старый loopback после переключения на наушники продолжает «слушать» замолчавшее устройство.

## Watch out
Когда в Windows ничего не играет, loopback не отдаёт пакеты вовсе — кадров нет. Это нормально (так было и с audify/RtAudio): страница сама перестраивает буфер при недоборе.
Микширование в моно: (L+R)/2 плюс центральный канал ×0,707, если он есть в маске (5.1/7.1 — там диалоги).

In the code: src/capture.rs — Loopback::open, Loopback::pump, run (проверка default_device_id раз в DEVICE_CHECK).
