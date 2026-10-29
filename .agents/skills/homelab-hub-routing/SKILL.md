---
name: homelab-hub-routing
description: >-
  Manage and troubleshoot Oracle Homelab backend services, Cloudflare Worker API routing,
  multi-package client telemetry disambiguation, and private vault key retrieval.
---

# Homelab Hub & Cloudflare Routing Architecture

## Overview
This skill guides maintenance and development across the Homelab Hub and Cloudflare edge proxies (`odm_hub_live`, `homelab_hub`).

## 1. Private Credential Discovery
- Before prompting the user for API keys or service credentials, search:
  1. `K:\private\xiyj\` for Google Cloud/BigQuery JSON credentials, Firebase service accounts, and Cloudflare tokens.
  2. `E:\Downloads\google-services.json` for client firebase configs.
  3. `E:\02_Projects\Oracle_Home\homelab_hub` for local environment templates.

## 2. Multi-Package Analytics Disambiguation
- When routing telemetry or license validation across different app versions:
  - Both Flutter (`com.personal.aurora_downloader`) and Kotlin Native (`com.odm.app`) report to `odm.ahjie521.store`.
  - Always enforce and validate the `?package=` query parameter in Cloudflare Workers and backend handlers to prevent mixing analytics datasets.
