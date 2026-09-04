---
title: Guide
---

# Developer guide

Start with print mode. Every feature must work there first.

## Catalog cache

The catalog cache stores the provider model list with a timestamp.
A failed refresh may use the last cached list, marked stale.

## Sessions

Sessions are stored beside native sessions for both execution paths.
