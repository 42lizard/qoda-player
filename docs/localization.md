# Localization

Qoda ships English and German. The UI follows Qt's ordered user UI-language
preferences (`Qt.locale().uiLanguages`). German regional preferences such as
`de-DE`, `de-AT` and `de-CH` use German; English uses English. If no supported
preference is available, including C/POSIX without an explicit UI-language
preference, English is the default. The usual
session `LANG`, `LC_MESSAGES`, `LC_ALL` and `LANGUAGE` settings are interpreted by
Qt; restart the UI after changing them. Installed services inherit the user
service manager's session environment. Qoda does not change your system locale.

Number and date formatting use the relevant default Qt locale independently of
the text language. An unsupported UI language can therefore show English text
with the user's local number/date conventions. On Linux, use an installed UTF-8
locale; Quickshell/Qt may normalize an invalid libc locale before loading QML.

## Translation seam

`ui/I18n.qml` owns language selection and presentation. English source messages
are the catalog keys in `ui/i18n/de.json`; values are German. `German.js` is the
checked-in generated asset, so source launches and installed packages need no
translation compiler, native plugin, network request or runtime file loading.
`Catalog.js` provides fallback, parameter substitution and the small adapter for
existing English backend messages. This keeps the versioned backend protocol,
queue/session identities, playback intent and machine-readable values unchanged.

QML labels use `I18n.t("Source sentence", [parameters])`. Use whole sentences
with numbered `%1` placeholders; never substitute by repeated string replacement.
`I18n.count(one, many, count)` handles singular/plural, and `I18n.number` formats
numbers for display. State labels use `I18n.state` while comparisons and outgoing
commands retain their original values. Brand names and technical codec identifiers
remain unchanged.

Only explicit displayed backend status/error/reason fields cross `I18n.message`.
Exact English sentences and anchored known format templates are translated;
unknown/new messages retain their original English details. The adapter preserves
parameters, translates known nested error explanations, and bounds matching and
nesting. It never walks or translates a snapshot recursively. Plex titles, artist
credits, server/library names, user-entered playlist titles, export paths and
protocol tokens are opaque data. Keep text containing provider data plain text.
Console/CLI diagnostics and provider metadata are outside UI translation.

## Updating translations

1. Add the complete English source sentence and German translation to `de.json`.
   Preserve every numbered or Rust-format placeholder, including repeated ones.
2. Use the presentation functions at the display point. Do not translate at
   command admission or while storing catalog/session data. If a backend message
   changes, update its source key and its tests together; fallback remains English.
3. Run `python3 tools/check-localization.py --write` to regenerate `German.js`.
4. Run `python3 tools/check-localization.py --runtime` and the QML suite. The full
   `tools/check.sh` does this automatically. Catalog/placeholder or generated-asset
   drift fails checks. Fresh Qt processes verify locale precedence and fallback;
   real German controls verify display and unchanged commands. Existing protocol
   tests retain their English fixtures under `LANGUAGE=en LC_ALL=C`.
5. Use `LANGUAGE=de LC_ALL=en_US.UTF-8 python3 tools/check-layout.py --output /tmp/qoda-de`
   with an installed UTF-8 locale to inspect the fictional full-window layouts.
   This creates no account connection or audio playback. German text must wrap or
   fit; retain full tooltips for intentionally elided metadata.

User and pacman installation copy the catalogs with the UI. The source installer
allows only reviewed localization filenames and rejects unexpected files or
symlinks. Package checks require the localization assets in the installed release.

References: [Qt UI-language preferences](https://doc.qt.io/qt-6/qml-qtqml-locale.html#uiLanguages-prop)
and [Qt locale formatting](https://doc.qt.io/qt-6/qml-qtqml-locale.html).
