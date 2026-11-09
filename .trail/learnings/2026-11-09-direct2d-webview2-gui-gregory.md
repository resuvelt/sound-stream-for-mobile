---
title: "Окно и панель трея рисуем сами через Direct2D, без WebView2 и GUI-библиотек"
author: Gregory
date: 2026-11-09
branch: main
cost: { minutes: 29, tokens: 640841 }
id: l-6287ea02
scope: repo
status: active
src: agent
touches:
  - src/ui/gfx.rs
  - src/ui/kit.rs
  - src/ui/mod.rs
  - src/ui/window.rs
  - src/ui/flyout.rs
  - README.md
  - build.rs
  - src/tray.rs
  - src/main.rs
  - public/icon-512.png
---

## Decided
Интерфейс (главное окно с QR-кодом и панель у значка в трее) нарисован вручную на Direct2D + DirectWrite в стиле Windows 11: Mica у окна, Acrylic у панели, скругления через DWM, тёмная/светлая тема из реестра. Так exe остаётся одним файлом без рантаймов: WebView2 добавил бы зависимость от WebView2 Runtime (на Windows 10 LTSC/N его может не быть) и ~100 МБ памяти на процессы Edge, а egui/iced/Slint — свой цикл событий вместо готового цикла трея, текст без ClearType-качества и мегабайты в exe. Окно открывается при ручном запуске и по повторному запуску exe; автозапуск пишется с --autostart и показывает только значок.

## Watch out
- Mica/Acrylic видны, только если DwmExtendFrameIntoClientArea(-1) и холст Direct2D с D2D1_ALPHA_MODE_PREMULTIPLIED очищается в прозрачный цвет; сглаживание текста тогда только GRAYSCALE (ClearType на прозрачном фоне не работает). Системный фон есть с Windows 11 22621; ниже — сплошной цвет.
- DestroyWindow сам присылает WM_ACTIVATE(WA_INACTIVE): панель трея закрывается через dismiss() с флагом closing, иначе закрывалась бы дважды.
- Все обработчики берут состояние окна через try_borrow_mut: SetWindowPos, ReleaseCapture и т.п. синхронно зовут ту же оконную процедуру, а паника в extern "system" — аварийное завершение.
- WM_MOUSELEAVE в windows-sys лежит в Win32::UI::Controls, а не в WindowsAndMessaging.
- Проверять без мыши: PostMessage окну класса sound-stream.tray сообщения WM_APP+1 с lParam WM_LBUTTONUP открывает/закрывает панель (повторный «клик» её прячет); снимок — CopyFromScreen по DWMWA_EXTENDED_FRAME_BOUNDS, временно сделав окно TOPMOST.

In the code: src/ui/gfx.rs (холст и примитивы, QR, логотип), src/ui/kit.rs (палитра Fluent, кнопки, переключатель, мышь), src/ui/window.rs, src/ui/flyout.rs, src/ui/mod.rs (decorate — DWM, info — снимок состояния, perform — действия); src/tray.rs — клик по значку → ui::flyout::toggle, двойной клик и второй запуск → ui::window::show.
