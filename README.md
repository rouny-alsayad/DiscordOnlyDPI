# DiscordOnlyDPI

**Discord-only DPI bypass for Windows, written in Rust.**

DiscordOnlyDPI routes only Discord through a local ByeDPI-based proxy while leaving games, browsers, launchers, and the rest of Windows on the normal network path.

> This project is **not a system-wide VPN**. It does not enable a Windows proxy, change the default route, install WinDivert, or install a kernel network driver.

**Current version:** `v0.9.3`

[العربية](#العربية) • [English](#english)

---

# English

## What is DiscordOnlyDPI?

DiscordOnlyDPI is a Windows utility designed to make Discord connectivity work through a local ByeDPI path without sending game traffic through the same path.

The main goal is simple: **affect Discord only**.

### Main features

- Discord-only local SOCKS5 proxy on `127.0.0.1:17891`.
- ByeDPI backend bound to loopback only on `127.0.0.1:17892`.
- Temporary local HTTP CONNECT proxy for Discord updates on `127.0.0.1:17893`.
- No Windows System Proxy changes.
- No global DNS changes.
- No Windows route changes.
- No WinDivert.
- No kernel network driver.
- No DLL injection into Discord or games.
- Automatic Discord Host + Module updates.
- SHA-256 verification for downloaded Discord update packages.
- Automatic Minimize to Tray.
- Custom application / taskbar / tray icon.
- One diagnostic log file only: `last-run.log`.

## How it works

1. DiscordOnlyDPI starts the bundled/downloaded ByeDPI engine on loopback only.
2. It starts a local SOCKS5 frontend for Discord.
3. It checks Discord's official distribution manifest.
4. If a newer Discord Host version is available, it downloads the official `full.distro` package through the local update path.
5. The package SHA-256 is verified before installation.
6. Discord modules from the same official manifest are downloaded and installed for the matching Host version.
7. The temporary update proxy is stopped before Discord launches.
8. Discord is launched with an explicit Chromium proxy argument:

```text
--proxy-server=socks5://127.0.0.1:17891
```

9. DiscordOnlyDPI stays running while Discord is open.
10. When Discord exits, the local proxy tasks and ByeDPI process are stopped.

## Game-safe isolation

DiscordOnlyDPI is designed so game traffic does not pass through the Discord proxy path.

The program does **not** modify:

- the Windows default route;
- the Windows System Proxy;
- system DNS settings;
- firewall rules;
- game processes;
- anti-cheat processes.

The temporary HTTP update proxy is used only while DiscordOnlyDPI itself checks/downloads Discord updates, then it is stopped before Discord is launched.

Discord is also launched with inherited `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY` environment variables removed, so only the explicit Discord SOCKS proxy is used.

> No software can guarantee compatibility with every anti-cheat implementation. This project's architecture avoids system-wide packet interception and does not intentionally interact with games or anti-cheat software.

## Automatic Discord updates

DiscordOnlyDPI manages Discord updates itself because Discord's built-in updater may not use the same Electron proxy path.

On startup it checks the official Discord manifest. If the installed Host version is old, DiscordOnlyDPI downloads and installs the current Host package first, then synchronizes the modules that belong to that exact Host version.

Existing compatible modules are reused. Packages are not mixed between different Discord Host versions.

This means normal Discord Host and Module releases should not require a new DiscordOnlyDPI release **as long as Discord keeps the same distribution manifest/package format**.

## Minimize to Tray

Press the normal **Minimize** button on the DiscordOnlyDPI console window.

The window will:

- disappear from the Windows taskbar;
- stay available in the Notification Area / tray (`^` near the clock);
- restore when you click the DiscordOnlyDPI tray icon.

The executable, taskbar window, and tray use the DiscordOnlyDPI icon from:

```text
assets/DiscordOnlyDPI.ico
```

## Diagnostic log

DiscordOnlyDPI keeps **one log file only**:

```text
%LOCALAPPDATA%\DiscordOnlyDPI\last-run.log
```

Every launch truncates the previous run and records only the current/latest run.

The log includes DiscordOnlyDPI messages and relevant child-process output, making it the first file to share when troubleshooting a crash, failed update, missing module, or connection problem.

## Usage

Download or build the Windows x64 executable and run:

```text
DiscordOnlyDPI-v0.9.3-windows-x64.exe
```

Keep DiscordOnlyDPI running while using Discord. You can minimize it to the tray.

### Experimental Voice TCP mode

```text
DiscordOnlyDPI-v0.9.3-windows-x64.exe --voice-tcp
```

This adds WebRTC IP-handling switches intended to avoid non-proxied UDP during testing.

**Voice/WebRTC support is experimental and is not guaranteed.** Discord voice and screen sharing may use networking behavior different from normal HTTPS/WebSocket traffic.

## Files and local data

DiscordOnlyDPI uses:

```text
%LOCALAPPDATA%\DiscordOnlyDPI\bin\ciadpi.exe
%LOCALAPPDATA%\DiscordOnlyDPI\last-run.log
```

The project does not need to read Discord tokens or account credentials.

## ByeDPI

DiscordOnlyDPI uses [ByeDPI](https://github.com/hufrea/byedpi) as the local DPI-bypass engine.

The current code expects ByeDPI `v0.17.3` and verifies the downloaded archive/executable against the SHA-256 value defined in the source before using it.

## Build from source

### Requirements

- Rust toolchain
- Windows 10/11 for normal native use
- For Linux cross-compilation: `cargo-xwin`
- `llvm-rc` when embedding the Windows icon from Linux

### Windows

```bash
cargo build --release
```

### Linux -> Windows x64

The repository includes:

```bash
bash build-windows.sh
```

The script runs formatting/checks, Clippy, and the Windows MSVC release build. It reads the package version from `Cargo.toml` and outputs:

```text
dist/DiscordOnlyDPI-v<version>-windows-x64.exe
```

Do not commit build output such as `target/` or `dist/` to the repository. GitHub Releases is the better place for compiled `.exe` files.

## Project structure

```text
DiscordOnlyDPI/
├── assets/
│   └── DiscordOnlyDPI.ico
├── src/
│   └── main.rs
├── build.rs
├── build-windows.sh
├── tools/
│   └── rc.exe
├── Cargo.toml
├── Cargo.lock
├── README.md
└── .gitignore
```

## Privacy and security notes

- Proxy listeners bind to `127.0.0.1`, not public interfaces.
- Discord traffic is explicitly proxied; games are not intentionally routed through the proxy.
- Update packages are downloaded from Discord's manifest-provided URLs and checked with the manifest SHA-256.
- ByeDPI is hash-checked before use.
- No Discord token/credential extraction is part of the project.

## Limitations

- Discord can change its updater, manifest, package format, module layout, or networking behavior at any time.
- Such a major upstream change may require a DiscordOnlyDPI update.
- Voice and screen sharing are more complex than normal Discord HTTPS traffic and may require additional work.
- Anti-cheat compatibility cannot be guaranteed universally.

## Disclaimer

DiscordOnlyDPI is an independent project and is not affiliated with, endorsed by, or maintained by Discord or ByeDPI.

Use it only where you are authorized to do so and in accordance with applicable rules and local law.

---

# العربية

## ما هو DiscordOnlyDPI؟

DiscordOnlyDPI هو برنامج Windows مكتوب بلغة Rust هدفه تشغيل **Discord فقط** عبر مسار محلي يعتمد على ByeDPI، بدون تمرير اتصال الألعاب وباقي برامج ويندوز عبر نفس المسار.

الهدف الأساسي للمشروع هو: **التأثير على Discord فقط**.

> البرنامج **ليس VPN للنظام بالكامل**، ولا يفعّل System Proxy على ويندوز، ولا يغيّر Default Route، ولا يستخدم WinDivert، ولا يثبت Network Driver داخل النظام.

## الميزات الرئيسية

- SOCKS5 محلي خاص بـDiscord على `127.0.0.1:17891`.
- ByeDPI يعمل على loopback فقط على `127.0.0.1:17892`.
- HTTP CONNECT مؤقت لتحديث Discord على `127.0.0.1:17893`.
- بدون تغيير System Proxy.
- بدون تغيير DNS العام لويندوز.
- بدون تغيير Routes.
- بدون WinDivert.
- بدون Kernel Network Driver.
- بدون DLL Injection داخل Discord أو الألعاب.
- تحديث تلقائي لـDiscord Host والـModules.
- فحص SHA-256 لحزم تحديث Discord قبل تثبيتها.
- Minimize to Tray.
- لوقو مخصص للـEXE والـTaskbar والـTray.
- ملف Log واحد فقط للتشخيص: `last-run.log`.

## كيف يعمل البرنامج؟

1. يشغّل DiscordOnlyDPI محرك ByeDPI محليًا على loopback فقط.
2. يشغّل SOCKS5 محلي مخصص لاتصال Discord.
3. يفحص الـmanifest الرسمي الخاص بتوزيعات Discord.
4. إذا كان هناك Discord Host أحدث، ينزّل `full.distro` الرسمي عبر مسار التحديث المحلي.
5. يتحقق من SHA-256 قبل التثبيت.
6. ينزّل Modules المطابقة لنفس إصدار الـHost من الـmanifest الرسمي ويثبتها.
7. يغلق HTTP update proxy قبل تشغيل Discord.
8. يشغّل Discord مع:

```text
--proxy-server=socks5://127.0.0.1:17891
```

9. يبقى DiscordOnlyDPI شغالًا أثناء تشغيل Discord.
10. عند إغلاق Discord، يوقف البروكسي المحلي وByeDPI.

## عزل الألعاب Game-Safe Isolation

DiscordOnlyDPI مصمم بحيث اتصال اللعبة لا يمر عبر بروكسي Discord.

البرنامج لا يعدّل:

- Default Route الخاص بويندوز.
- System Proxy.
- DNS العام للنظام.
- Firewall Rules.
- عمليات الألعاب.
- عمليات Anti-Cheat.

HTTP update proxy يعمل فقط أثناء فحص أو تنزيل تحديثات Discord ثم يتم إيقافه قبل تشغيل Discord.

كما أن البرنامج يزيل متغيرات `HTTP_PROXY` و`HTTPS_PROXY` و`ALL_PROXY` من بيئة عملية Discord، ويعتمد فقط على SOCKS المحدد صراحةً لـDiscord.

> لا يمكن لأي برنامج ضمان التوافق مع كل أنظمة Anti-Cheat الموجودة. لكن تصميم DiscordOnlyDPI لا يعتمد على اعتراض Packets للنظام كاملًا ولا يتعامل عمدًا مع عمليات الألعاب أو Anti-Cheat.

## التحديث التلقائي لـDiscord

DiscordOnlyDPI يدير تحديث Discord بنفسه لأن Discord updater الداخلي قد لا يستخدم نفس مسار Electron proxy.

عند كل تشغيل، البرنامج يفحص الـmanifest الرسمي. إذا كان Discord Host المثبت قديمًا، ينزّل Host الجديد أولًا، ثم يزامن Modules المطابقة لنفس الإصدار بالضبط.

الـModules الموجودة بنفس الإصدار يتم إعادة استخدامها، ولا يتم خلط Modules بين Host versions مختلفة.

بالتالي تحديثات Discord الطبيعية لا يفترض أن تحتاج نسخة جديدة من DiscordOnlyDPI **طالما Discord لم يغيّر صيغة الـmanifest أو `full.distro` أو بنية التحديث بشكل جذري**.

## Minimize to Tray

اضغط زر **Minimize** العادي على نافذة DiscordOnlyDPI.

سيقوم البرنامج بـ:

- الاختفاء من الـTaskbar.
- البقاء داخل Notification Area / السهم `^` بجانب الساعة.
- استعادة النافذة عند الضغط على أيقونة DiscordOnlyDPI.

الأيقونة المستخدمة داخل المشروع:

```text
assets/DiscordOnlyDPI.ico
```

## ملف التشخيص Log

البرنامج يحتفظ **بملف Log واحد فقط**:

```text
%LOCALAPPDATA%\DiscordOnlyDPI\last-run.log
```

عند كل تشغيل يتم مسح محتوى التشغيل السابق، ثم تسجيل تشغيل المرة الحالية فقط.

هذا الملف يسجل رسائل DiscordOnlyDPI ومخرجات مهمة من العمليات التابعة، لذلك هو أول ملف يجب مشاركته إذا حدث Crash أو فشل تحديث أو Module ناقص أو مشكلة اتصال.

## الاستخدام

شغّل:

```text
DiscordOnlyDPI-v0.9.3-windows-x64.exe
```

اترك البرنامج شغالًا أثناء استخدام Discord. يمكنك تصغيره إلى الـTray.

### وضع Voice TCP التجريبي

```text
DiscordOnlyDPI-v0.9.3-windows-x64.exe --voice-tcp
```

هذا الوضع يضيف WebRTC IP-handling switches لمحاولة تجنب non-proxied UDP أثناء الاختبار.

**دعم Voice/WebRTC تجريبي وليس مضمونًا.** الصوت وScreen Share في Discord قد يستخدمان سلوك شبكة مختلفًا عن HTTPS/WebSocket العادي.

## الملفات والبيانات المحلية

DiscordOnlyDPI يستخدم:

```text
%LOCALAPPDATA%\DiscordOnlyDPI\bin\ciadpi.exe
%LOCALAPPDATA%\DiscordOnlyDPI\last-run.log
```

المشروع لا يحتاج إلى قراءة Discord tokens أو بيانات تسجيل الدخول الخاصة بالحساب.

## ByeDPI

يستخدم DiscordOnlyDPI مشروع [ByeDPI](https://github.com/hufrea/byedpi) كمحرك DPI bypass محلي.

الكود الحالي يعتمد على ByeDPI `v0.17.3` ويتحقق من SHA-256 قبل استخدام الملف الذي تم تنزيله.

## البناء من المصدر

### المتطلبات

- Rust toolchain.
- Windows 10/11 للاستخدام الطبيعي.
- عند البناء من Linux إلى Windows: `cargo-xwin`.
- `llvm-rc` لتضمين أيقونة Windows عند cross-compiling من Linux.

### البناء على Windows

```bash
cargo build --release
```

### البناء من Linux إلى Windows x64

استخدم السكربت الموجود بالمشروع:

```bash
bash build-windows.sh
```

السكريبت يشغّل formatting/checks وClippy ثم Windows MSVC release build، ويقرأ رقم النسخة تلقائيًا من `Cargo.toml`.

الناتج يكون داخل:

```text
dist/DiscordOnlyDPI-v<version>-windows-x64.exe
```

يفضل عدم رفع `target/` أو `dist/` إلى Git. استخدم GitHub Releases لنشر ملفات `.exe` الجاهزة.

## بنية المشروع

```text
DiscordOnlyDPI/
├── assets/
│   └── DiscordOnlyDPI.ico
├── src/
│   └── main.rs
├── build.rs
├── build-windows.sh
├── tools/
│   └── rc.exe
├── Cargo.toml
├── Cargo.lock
├── README.md
└── .gitignore
```

## الخصوصية والأمان

- البروكسيات المحلية تستمع على `127.0.0.1` فقط.
- Discord فقط يتم تشغيله مع SOCKS proxy صريح.
- الألعاب لا يتم تمريرها عمدًا عبر مسار البروكسي.
- حزم تحديث Discord يتم تنزيلها من الروابط الموجودة في الـmanifest الرسمي والتحقق من SHA-256 الخاص بها.
- ByeDPI يتم التحقق من Hash الخاص به قبل الاستخدام.
- المشروع لا يحتوي وظيفة لاستخراج Discord token أو credentials.

## القيود

- Discord يستطيع تغيير updater أو manifest أو package format أو module layout أو سلوك الشبكة في أي وقت.
- إذا حصل تغيير جذري من Discord، قد نحتاج تحديث DiscordOnlyDPI.
- Voice وScreen Share أعقد من اتصال Discord العادي وقد تحتاج تطوير إضافي.
- لا يمكن ضمان التوافق مع جميع أنظمة Anti-Cheat بشكل مطلق.

## تنبيه

DiscordOnlyDPI مشروع مستقل وغير تابع أو معتمد أو مُدار من Discord أو ByeDPI.

استخدمه فقط حيث يكون استخدامه مسموحًا، وبما يتوافق مع القوانين والقواعد المطبقة عليك.
