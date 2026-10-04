# DiscordOnlyDPI

أداة Windows/Rust لتشغيل **Discord فقط** عبر ByeDPI بدون WinDivert وبدون تغيير Proxy أو DNS للنظام.

## الهدف

- Discord فقط يمر عبر SOCKS5 المحلي.
- الألعاب والمتصفح وباقي البرامج تبقى على الإنترنت العادي.
- لا يتم تثبيت kernel driver.
- لا يتم تغيير System Proxy.
- لا يتم تغيير DNS الخاص بويندوز.
- لا يتم حقن DLL داخل Discord أو الألعاب.

## كيف تعمل v0.1

1. عند أول تشغيل، الأداة تنزّل ByeDPI v0.17.3 الرسمي إلى:
   `%LOCALAPPDATA%\DiscordOnlyDPI\bin\ciadpi.exe`
2. تتحقق من SHA-256 قبل تشغيله.
3. تشغّل ByeDPI على loopback فقط: `127.0.0.1:17892`.
4. تشغّل SOCKS5 frontend خاص بالأداة على `127.0.0.1:17891`.
5. DNS المطلوب من Discord يتم حله عبر DoH داخل الأداة، بدون تغيير DNS ويندوز.
6. تغلق Discord الموجود ثم تشغّل أحدث `Discord.exe` مباشرة مع:
   `--proxy-server=socks5://127.0.0.1:17891`
7. عند إغلاق Discord، تتوقف الأداة وByeDPI.

## الاستخدام

شغّل:
`DiscordOnlyDPI-v0.1-windows-x64.exe`

اترك نافذة الأداة مفتوحة أثناء استخدام Discord.

### اختبار الصوت عبر TCP (تجريبي)

من CMD:
`DiscordOnlyDPI-v0.1-windows-x64.exe --voice-tcp`

الوضع الافتراضي لا يجبر WebRTC على البروكسي. هذا متعمد حتى نختبر الصوت أولاً بدون كسر الاتصال.

## حدود v0.1

- واجهة Discord والرسائل وHTTPS/WebSocket تمر عبر البروكسي.
- Discord voice/WebRTC UDP لا يمر تلقائياً عبر SOCKS5 في الوضع الافتراضي.
- تحديث Discord يتم تجاوزه حالياً بتشغيل Discord.exe من آخر مجلد app-* مباشرة؛ updater مخصص سيضاف بعد اختبار النسخة الأولى.
- توافق كل Anti-Cheat لا يمكن ضمانه نظرياً، لكن التصميم لا يثبت WinDivert ولا driver ولا system-wide VPN/proxy.

## البناء

على Windows مع Rust:
`cargo build --release`

على Linux مع cargo-xwin:
`cargo xwin build --release --target x86_64-pc-windows-msvc`
