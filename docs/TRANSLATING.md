# Interface languages

The language menu offers English, Português (Brasil), Português (Portugal), Deutsch and
Русский. The selected language is stored with the preferences. Empty preferences follow
the operating system, including the Windows locale and regional Portuguese codes.

Existing `tr("English", "Русский")` calls remain valid. Russian keeps its contextual inline
text; `locales/ru.json` is a reference catalogue. The other languages use embedded UTF-8
JSON catalogues keyed by the English text. No service, model, download or network request
is used to translate text while the editor runs. Future missing entries fall back to English.

`locales/en.json` lists the source labels. Run `python3 tools/check-locales.py` after changing
interface text; it checks source coverage, empty entries and preservation of format fields,
environment-variable names and URLs. Add new command-palette labels too.

The Portuguese variants have separate wording (for example Salvar/Guardar and
arquivo/ficheiro); changes in one catalogue do not silently change the other. The first
catalogues cover 2,065 source labels, including command-palette macros. Menus and common
editor terminology have received an initial review. Long help/error text and visual fit
still need language-speaker and in-app review; coverage is not a claim of linguistic accuracy.
