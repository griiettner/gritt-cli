# Keys

Gritt follows ADR-008. A provider key is resolved in this order and no
other:

1. The operating system keychain entry named by the profile's
   `keychain_service_entry` (macOS Keychain, Windows Credential Manager, or
   a Linux Secret Service provider).
2. The environment variable named by the profile's `env_var_name`.

## Storing a key

```bash
gritt key-set openrouter
```

reads one line from stdin and writes it to the keychain. It is never written
to a file. Pipe it in to avoid the terminal echo:

```bash
pbpaste | gritt key-set openrouter
```

If no keychain is available, set the environment variable instead; that
mode is fully supported.

## What never happens

- A config file never holds a key value. Loading one that does fails with a
  `secret in config` error that does not echo the value.
- Keys never appear in logs, errors, fixtures, transcripts, telemetry,
  session events, connector diagnostics, or the content log. Every path
  that could carry provider or connector output passes through a redactor
  that knows the active key values and every credential-like variable in
  the process environment.
- Native shell tools run without credential variables in their environment
  (names containing `KEY`, `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, or
  `CREDENTIAL`, plus the configured profile variables).
- External connectors keep their environment, because they own their own
  credentials (ADR-010); anything they echo back is redacted instead.

`gritt config` and `gritt doctor` report whether a key is available for a
profile. Neither prints a value.
