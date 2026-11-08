# Workspace timezone identifiers

`timezones.txt` is a bounded list of timezone identifiers, generated on 2026-11-05 from the installed Node runtime's `Intl.supportedValuesOf("timeZone")` plus the canonical names returned by `Intl.DateTimeFormat` and UTC/Etc aliases. These are identifiers, not a copied timezone-rule database; no package or network request was added.

The server validates labels against this list. Reminders persist an absolute UTC second and never calculate offsets using the label. The desktop validates local wall times using its existing Intl runtime before sending UTC. Newly introduced or renamed zones require refreshing this list; unsupported labels fail explicitly rather than silently falling back to another zone.
