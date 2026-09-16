# Changelog

All notable changes to `nb-api` are documented here. Pre-1.0 releases may
break the API without deprecation.

## [0.4.0]

Breaking pre-1.0 release. See `documentation/migration-0.4.0.md` for the
full migration guide.

- Text-first wire types: `ByteString` removed entirely; all structured
  textual fields are JSON `String` (`ShowNote` title/body/source,
  `BodyFragment` bytes plus `start_byte`/`end_byte` offsets into `source`,
  `NoteLine` text, bare-text `LineEdit` content, search hits, substring
  pattern/replacement).
- Text-only surface: non-UTF-8 files fail with typed `NonUtf8`; raw access
  via `NbClient::read_note_source_bytes`. Base64 dependency dropped.
- Document-level EOL: per-line terminators replaced by
  `ShowNoteLines { eol: Option<LineEol>, has_final_eol }` with
  first-supported-occurrence detection; bare `\r` preserved verbatim;
  exact-span line anchors; edits on `eol: None` documents adopt `Lf`.
- nb-faithful identity: title-mangled filenames (Unicode preserved,
  local-time titleless, `-N` collisions; opaque `{epoch}-{seq}` names
  removed), folder `.index` maintenance with stable positional ids
  (append / blank-on-delete / in-place rename / blank+append across
  folders), numeric `<folder>/<id>` selectors and `numeric_id` on outcomes
  and reads. Same-folder growing renames refused at plan validation.
- Cross-process `.index` safety: `O_APPEND` appends, atomic range removal
  where the platform offers it (Linux collapse; documented residual
  elsewhere), nonce lock with heartbeat (`IndexLockTimeout` on contention).
- Errors: new `NonUtf8` and `IndexLockTimeout` variants.
