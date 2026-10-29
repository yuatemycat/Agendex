---
name: android-download-engine
description: >-
  Build, debug, or optimize Android high-performance download engines, multi-part chunk downloads,
  MediaStore public storage publishing, and WebView media sniffers. Use when developing Android
  downloaders, handling range requests, scoped storage publication, or intercepting streaming media.
---

# Android High-Performance Download Engine & Media Sniffer

## Overview
This skill provides production-tested patterns for Android downloading:
1. Multi-part chunk downloading via HTTP `Range` headers.
2. Graceful fallback from HTTP 206 (Partial Content) to HTTP 200 (Single-stream).
3. Android Scoped Storage & MediaStore public publishing.
4. WebView media sniffing with MIME validation.

## 1. Multi-Part Chunk Downloader Pattern
- **Pre-flight Probe**: Send a `HEAD` or `GET` with `Range: bytes=0-0` to inspect `Accept-Ranges: bytes` and `Content-Length`.
- **Chunk Sizing**: Divide content into 4-8 parallel worker chunks for files > 10MB; cap worker count based on CPU cores and network type.
- **RandomAccessFile Preallocation**: Preallocate target file size using `RandomAccessFile.setLength(totalBytes)` to prevent storage fragmentation and out-of-space crashes midway.
- **Rate Limiting**: Implement a Token Bucket algorithm to throttle combined throughput across active download threads.

## 2. Public Storage Publication (MediaStore)
- Stream chunks into app cache (`context.cacheDir/chunks/<id>`).
- Reassemble and publish to `MediaStore.Downloads` using the 2-phase `IS_PENDING` workflow (see `android-storage-invariants.md`).
- Ensure target directory is `Download/ODM Downloader` rather than isolated app sandboxes (`/Android/data/...`).

## 3. WebView Media Sniffer Pattern
- Hook `WebViewClient.shouldInterceptRequest(view, request)`:
  - Filter out ad domains, analytics scripts, and static assets (`.js`, `.css`, `.png`, `.svg`).
  - Sniff video extensions (`.mp4`, `.m3u8`, `.mpd`, `.webm`).
  - For ambiguous URLs, issue a lightweight background `HEAD` request to inspect `Content-Type` (`video/*`, `application/x-mpegURL`, `application/vnd.apple.mpegurl`).
- Propagate captured media metadata (URL, headers, cookies, referer) to the download queue.
