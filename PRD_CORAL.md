# PRD — Corral: Laravel PHP Dev Server untuk Windows (Rust + Tauri 2)

**Versi dokumen:** 1.0
**Tanggal:** 10 November 2026
**Kode produk:** `corral`
**Basis riset:** hasil reverse engineering Laravel Herd 2.1.1 (Windows x64), lihat lampiran §17.1
**Platform:** Windows 10 (1809+) / Windows 11, **x64 only**, toolchain MSVC
**Status:** draft untuk implementasi

---

## 1. Executive Summary

**Corral** adalah aplikasi desktop Windows yang menyediakan environment PHP + Laravel siap pakai tanpa Docker, tanpa WSL, tanpa dependency chain: nginx, PHP (multi-versi sebagai FastCGI), MySQL/MariaDB, PostgreSQL, Redis, mail catcher, Reverb/queue worker, TLS lokal, dan domain `.test` — semuanya diorkestrasi oleh satu binary Rust + UI WebView2.

**UVP:** "Herd-class DX dengan footprint Rust." Installer < 8 MB, RAM idle < 90 MB, cold start < 1,2 s, dan setiap proses anak disupervisi dengan Windows Job Object sehingga tidak ada zombie process setelah app ditutup. Runtime besar (PHP per versi, database engine) diunduh on-demand dengan verifikasi tanda tangan Ed25519, bukan dibundel — sehingga installer tetap kecil dan update PHP tidak butuh instal ulang aplikasi.

**Yang membedakan dari Herd (hasil RE):**

| Aspek | Herd 2.1.1 (teramati) | Corral (desain) |
|---|---|---|
| Privileged helper | Windows service `.NET 8` + `HttpListener` di `localhost:<port>` (HTTP) | Windows service **Rust** + **named pipe** dengan ACL SID user (tanpa port, tanpa HTTP) |
| Main process | Electron + V8 bytecode (`bytenode .jsc`) | Rust native, tanpa bytecode/JIT |
| Sertifikat | phpseclib3 (PHP) + `certutil -addstore` | `rcgen` (Rust) + CryptoAPI `CertAddCertificateContextToStore` |
| PHP server | `php-cgi.exe` FastCGI, port `base + versiInteger` | sama (dipilih karena Windows tidak punya php-fpm resmi) |
| Runtime | dibundel penuh (466 MB payload) | on-demand + signature verify |

**Pernyataan platform:** hanya `x86_64-pc-windows-msvc`. Tidak ada target macOS/Linux/ARM/mobile di v1.
**Pernyataan ukuran:** installer NSIS **< 8 MB**, `corral.exe` **< 6 MB**, bundle frontend **< 250 KB**.

---

## 2. Target Audience & Personas

**Persona utama — "Bertho", solo dev / small studio (primary):**
- Windows 11 x64, 16 GB RAM, SSD 512 GB, pakai Cursor/Claude Code + PowerShell 5.1.
- Butuh: `my-app.test` langsung jalan, ganti PHP 8.2↔8.5 dalam 1 klik, DB tanpa setup, mail catcher, HTTPS tanpa warning browser.
- Frustrasi sekarang: Laragon ribet, Docker makan 4–6 GB RAM, WSL path/latency aneh.

**Persona sekunder — "Tim 3–8 orang":**
- Butuh `herd.yml`-equivalent (`.corral.yml`) supaya config situs ikut repo, dan `corral link`/`corral park` di skrip onboarding.
- Butuh workspace per-branch dengan DB terpisah untuk QA paralel.

**Konteks perangkat:** laptop 8–16 GB RAM, sering buka 2–3 app Laravel sekaligus, tidak selalu punya hak admin (korporat) → helper elevation harus opsional dan app tetap **usable tanpa admin** (kecuali fitur hosts & trust cert).

---

## 3. Core Features

Prioritas: **P0** = v1 wajib, **P1** = v1.1, **P2** = v1.2, **P3** = backlog.
Kolom "Implementasi" menunjukkan mekanisme Rust/Tauri yang dipakai.

### 3.1 PHP runtime

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| PHP-1 | Manajer versi PHP | Unduh/side-by-side PHP 8.1–8.6 (dan 7.4 legacy opsional) ke `%DATA%\Corral\php\<ver>`; aktifkan per-global dan per-situs | Dua situs berbeda versi berjalan simultan; `corral php use 8.4` mengubah default < 300 ms | P0 | `reqwest` + manifest JSON, `zip` crate, `sha2` + `ed25519-dalek` verifikasi |
| PHP-2 | FastCGI server per versi | Spawn `php-cgi.exe` dengan `PHP_FCGI_CHILDREN`, bind `127.0.0.1:(base_php_port + verInt)` (mis. 9000+84) | `fcgi` health check OK; nginx `fastcgi_pass` sukses; port bebas dipilih otomatis bila bentrok | P0 | `tokio::process` + Job Object, `portscanner` internal (`TcpListener::bind`) |
| PHP-3 | Isolasi PHP per situs | Situs dapat override versi → generate server block dengan `$corral_sock_<ver>` | Ganti versi situs = reload nginx < 400 ms, tanpa restart service lain | P0 | template `minijinja` + `Command::new(nginx).arg("-s").arg("reload")` |
| PHP-4 | Editor php.ini terkelola | Set `memory_limit`, `upload_max_filesize`, `post_max_size`, `xdebug.*` lewat UI; tulis ulang `php.ini` per versi | Perubahan persist setelah restart; ada tombol "buka php.ini" | P0 | `infrastructure/php/ini.rs` (parse INI sederhana, bukan regex) |
| PHP-5 | Ekstensi | Pasang xdebug, redis, mongodb, imagick, intl dari manifest DLL per versi | Ekstensi muncul di `php -m` setelah restart versi terkait | P1 | unduh DLL + tulis `extension=` ke ini |
| PHP-6 | Node/nvm + Composer + Laravel installer | Kelola Node LTS 20/22/24 dan shim `composer`, `laravel`, `herd`-equivalent di PATH user | `node -v`, `composer -V`, `laravel new` jalan di PowerShell baru | P1 | Shims `.bat`/`.ps1` + `EnVar`-equivalent (registry `HKCU\Environment`) |

### 3.2 Web server & routing

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| WEB-1 | nginx terkelola | Bundel `nginx.exe` (build windows resmi), config di `%APPDATA%\Corral\etc\nginx`; master config + include-per-situs | `nginx -t` lolos sebelum apply; kalau gagal, config lama dipertahankan (rollback) | P0 | `infrastructure/nginx` + validasi pra-apply + atomic write (temp→rename) |
| WEB-2 | Domain `.test` otomatis | Setiap situs = `<folder>.test`, `www.<folder>.test`, wildcard `*.<folder>.test` | `http://my-app.test` resolve & serve tanpa edit manual | P0 | hosts-file writer lewat helper (§3.6) |
| WEB-3 | Document root pintar | Deteksi Laravel (`artisan` + `composer.json` → `laravel/framework`) → root = `public/`; proyek lain → root = folder | Situs baru langsung benar tanpa setting | P0 | `domain/site.rs::detect_doc_root()` |
| WEB-4 | Link / park / unlink | `corral link`, `corral park`, `corral unlink` + UI; support junction/symlink | Situs muncul/hilang < 1 s; aman untuk folder non-admin (pakai junction, bukan symlink) | P0 | `std::os::windows::fs::symlink_dir` fallback junction via `DeviceIoControl` atau `cmd /c mklink /J` |
| WEB-5 | Reverse proxy situs | Proxy `https://api.my-app.test` → `127.0.0.1:8080`, termasuk WebSocket upgrade | WebSocket proxy berfungsi; header `X-Forwarded-*` benar | P1 | template server block `proxy_pass` + `Upgrade` map |
| WEB-6 | Server snippets user | Folder `etc/nginx/server_snippets/*.conf` di-include otomatis | User bisa tambah header/rewrite tanpa menyentuh config app | P1 | include glob |
| WEB-7 | Port fallback & diagnosa | Kalau 80/443 terpakai atau masuk Windows "excluded port range" (Hyper-V/WSL), deteksi penyebab & tawarkan opsi (stop proses / pakai 8080) | Pesan error menyebut PID+nama proses pemakai port | P0 | `windows` crate `GetExtendedTcpTable` + `netsh int ipv4 show excludedportrange` |

### 3.3 Database & service

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| SVC-1 | Supervisor service | Satu `ServiceSupervisor` untuk nginx, php-cgi (N versi), mariadbd, postgres, redis-server, reverb, queue, scheduler | Semua anak masuk Job Object; tutup app = semua anak mati (0 zombie) | P0 | `CreateJobObject` + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` |
| SVC-2 | MariaDB/MySQL | Unduh + init datadir (`mysqld --initialize-insecure`), user `root` tanpa password, port 3306, socket aman `bind-address=127.0.0.1` | `DB_DATABASE` baru bisa dibuat dari UI; `.env` terisi otomatis | P0 | manifest + `--defaults-file` hasil template |
| SVC-3 | PostgreSQL | Sama, port 5432, `initdb -U postgres` | Connect via `psql` berhasil; UI bisa create DB | P1 | idem |
| SVC-4 | Redis | port 6379, `bind 127.0.0.1`, `maxmemory 256mb`, `appendonly no` | `redis-cli ping` → PONG | P1 | manifest (build Windows tervalidasi) |
| SVC-5 | MongoDB | port 27017 | Driver mongodb PHP connect | P2 | manifest |
| SVC-6 | Reverb (WebSocket) | `php artisan reverb:start` per situs, env `REVERB_APP_ID/KEY/SECRET` digenerate, proxy `reverb.<site>.test` | Echo/broadcast Laravel jalan lewat domain .test | P1 | worker supervisor + proxy block |
| SVC-7 | Queue & scheduler | `php artisan queue:work` / `schedule:work` dijalankan sebagai "worker situs"; auto-restart dengan backoff | Worker crash → restart ≤ 3× dalam 60 s lalu berhenti dengan status `degraded` | P1 | supervisor dengan policy restart + `Channel` log |
| SVC-8 | Multi-instance extra service | User bisa jalankan MySQL 8.0 dan MariaDB 11 di port berbeda, diberi nama | Dua instance hidup bersamaan; status & versi terlihat per instance | P2 | `ServiceInstance { id, kind, version, port, datadir }` |

### 3.4 TLS

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| TLS-1 | CA lokal + leaf per situs | `rcgen` buat CA (RSA-2048 atau ECDSA P-256, CN "Corral Local CA", 20 tahun) + leaf per host (SAN `dns` + `ip 127.0.0.1`, **≤ 396 hari** sesuai CA/B forum) | `https://my-app.test` hijau di Chrome/Edge/Firefox setelah trust | P0 | `rcgen` + `time` |
| TLS-2 | Trust/untrust 1 klik | Install CA ke `Root` (Windows) + `nss` Firefox best-effort; tombol revoke | Setelah trust, browser tidak warning; setelah revoke, warning kembali | P0 | CryptoAPI via `windows` crate (`CertOpenStore`/`CertAddEncCertificateToStore`) — atau fallback `certutil -addstore` |
| TLS-3 | Auto-renew | Cek expiry harian; leaf < 30 hari → regenerate + reload nginx; kedaluwarsa CA → notifikasi | Tester dengan cert 20 hari: regen otomatis terdeteksi di log | P1 | `tokio::time::interval` 24 h + job startup |
| TLS-4 | Toggle HTTPS per situs | Aktif/nonaktif TLS per situs, redirect 80→443 opsional | Toggle < 500 ms, tanpa restart app | P0 | regenerate server block + reload |

### 3.5 Mail, dumps, logs

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| MAIL-1 | Mail catcher SMTP | Server SMTP lokal `127.0.0.1:1025` (fallback port acak), simpan ke SQLite, UI list + preview HTML + attachment | `Mail::to(...)->send()` muncul di UI < 1 s | P0 | `tokio` + parser `mail-parser`; subset perintah `EHLO/HELO/MAIL/RCPT/DATA/RSET/QUIT` |
| MAIL-2 | Autofill `.env` | Tulis `MAIL_MAILER=smtp`, `MAIL_HOST=127.0.0.1`, `MAIL_PORT=1025`, `MAIL_FROM_ADDRESS=corral@test` | Situs Laravel baru langsung menangkap mail | P0 | `infrastructure/php/env.rs` (dotenv-safe edit, preserve komentar) |
| MAIL-3 | Dumps receiver | Terima `dump()` Laravel via protokol Symfony VarDumper server (`tcp://127.0.0.1:9912`, `VAR_DUMPER_FORMAT=server`) | `dump($x)` di route muncul realtime di panel Dumps | P1 | listener TCP + decode payload VarDumper (format teks + struktur) |
| LOG-1 | Log viewer | Tail realtime `storage/logs/laravel.log`, `nginx-error.log`, `php-fpm-<ver>.log`, `corral-*.log` | Streaming > 5 MB/s tanpa freeze UI (Channel + ring buffer 2 MB) | P0 | `tauri::ipc::Channel` + `notify` watcher + cap buffer |
| LOG-2 | Filter & search log | Filter level (error/warning) dan regex | Cari 10 MB log < 200 ms | P2 | index baris di memori (rope/ring), bukan tiap-tiap call |

### 3.6 Hosts, DNS, privilege

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| DNS-1 | Helper service privileged | Windows service `CorralHelper` (Rust, `windows-service` crate) yang **hanya** melakukan: tulis hosts block, trust/untrust cert, firewall rule, Defender exclusion. IPC = named pipe `\\.\pipe\corral-helper-v1` dengan ACL SID user saat ini | App non-admin bisa menambah domain tanpa UAC berulang; request dari user lain ditolak `ERROR_ACCESS_DENIED` | P0 | `windows-service` + `tokio::net::windows::named_pipe` + ACL via `SetSecurityInfo` |
| DNS-2 | Validasi ketat helper | Helper menolak: entri berisi IP mentah, TLD di luar allowlist (`test`, `local`, `wip`, `corral`), path cert di luar `%APPDATA%\Corral\ca`, request tanpa nonce+HMAC | Fuzz test 200 payload jahat: 0 yang mengubah hosts | P0 | validasi di sisi helper (bukan pemanggil) + `hmac`/`subtle` compare |
| DNS-3 | Hosts block atomik | Tulis blok di antara `# Corral generated Hosts. Do not change.` dan `# End Corral generated Hosts`; backup `hosts.bak`; rollback bila gagal | Crash di tengah tulis → hosts tetap valid setelah restart | P0 | tulis temp + `ReplaceFileW` + restore ACL/atribut read-only |
| DNS-4 | Mode tanpa admin | Kalau service tidak terpasang: app tetap jalan (situs via `127.0.0.1:<port>` dan `.test` via entri manual), dengan banner "pasang helper untuk domain otomatis" | Tanpa admin: web server + PHP + DB tetap berfungsi | P0 | degraded mode state machine |
| DNS-5 | (Opsional) DNS server lokal | Fallback tanpa helper: DNS server `127.0.0.1:5333` untuk user yang bisa set adapter DNS sendiri | Wildcard `*.test` resolve tanpa hosts | P3 | belum diimplementasi v1 |

### 3.7 Workspaces (paralel per branch)

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| WS-1 | Workspace dari git worktree | Buat workspace = `git worktree add` untuk branch, domain `<label>.<site>.test`, DB terpisah | 3 workspace satu repo hidup bersama, isolasi config/cache penuh | P1 | `git` CLI (`-C <repo> worktree add`) + parser `--porcelain` |
| WS-2 | Sinkron config workspace | Salin/turunkan `.env` dengan `DB_DATABASE`+`APP_URL` berbeda, `php artisan migrate --seed` opsional | Workspace baru siap pakai ≤ 15 s (repo sedang) | P1 | template env + job runner |
| WS-3 | Hapus/merge workspace | Hapus worktree + drop DB; "merge" = checkout branch ke situs utama | Tidak ada worktree/DB orphan setelah hapus (diverifikasi) | P2 | cleanup idempoten + ledger di SQLite |

### 3.8 Developer experience & integrasi

| # | Fitur | Detail | Kriteria terima | Prio | Implementasi |
|---|---|---|---|---|---|
| DX-1 | CLI `corral` | Subcommand: `link, unlink, park, unpark, php, node, db, open, tls, service, workspace, logs, mcp, doctor` | Semua subcommand punya `--json`; exit code ≠ 0 saat gagal | P0 | `herd-cli` crate → named pipe ke app (bukan HTTP) |
| DX-2 | Internal API untuk CLI | IPC app↔CLI lewat named pipe `\\.\pipe\corral-app-v1` (JSON-RPC minimal: `{id, method, params}`) | Latency < 5 ms per call; 100 call/detik stabil | P0 | `tokio` named pipe server + serde |
| DX-3 | `.corral.yml` per proyek | Deklarasi `php`, `node`, `https`, `services`, `env`, `document_root`, `workers` — divegakan ke repo | Situs dengan `.corral.yml` lain auto-apply saat link | P1 | `serde_yaml` + schema + validasi |
| DX-4 | Xdebug toggle | On/off global & per situs, `xdebug.client_host=127.0.0.1`, `client_port=9003`, `start_with_request=trigger` | Toggle < 300 ms, ekstensi ter-load versi terkait | P1 | tulis ini + restart php-cgi versi itu saja |
| DX-5 | Tinker & artisan runner | Tombol "Tinker", "Migrate", "Fresh" per situs dengan output streaming | Output streaming realtime, exit code terlihat | P1 | `Channel<String>` dari `tokio::process` stdout/stderr |
| DX-6 | Deteksi konflik | Deteksi Laragon/XAMPP/WSL/Docker yang memakai 80/443/3306 dan tawarkan resolusi | Pesan menyebut nama produk + PID | P1 | `GetExtendedTcpTable` + cek service name |
| DX-7 | Autostart & tray | Start minimized to tray, service auto-start on login, menu tray (situs terakhir, restart, quit) | Login → situs siap ≤ 3 s tanpa UAC | P1 | `tauri-plugin-autostart`, `TrayIconBuilder` |
| DX-8 | MCP server untuk AI agent | Expose tool MCP: `list_sites`, `create_site`, `switch_php`, `tail_log`, `db_query` (read-only) | Claude/Cursor bisa memanggil tool dan mendapat JSON valid | P2 | binary `corral-mcp` (stdio, `rmcp`-style JSON-RPC) |
| DX-9 | Update aplikasi | `tauri-plugin-updater` + signature; channel stable/beta | Update < 3 MB delta bila memungkinkan, rollback bila gagal start | P1 | updater + health check after update |
| DX-10 | Diagnostik `corral doctor` | Cek: versi komponen, port, hosts writable, cert trusted, service installed, PATH shims, WebView2 | Output tabel + saran perbaikan | P1 | satu command, hasil = struct |

### 3.9 Non-fitur (eksplisit di luar scope v1)

- Tidak ada integrasi akun/cloud (Forge/Laravel Cloud), tidak ada billing/licensing server.
- Tidak ada AI widget yang disuntik ke HTML situs (Herd Studio equivalent) di v1.
- Tidak ada dictation/whisper, tidak ada browser devtools bridge.
- Tidak ada dukungan macOS/Linux/ARM.
- Tidak ada virtual host non-`.test` TLD di luar allowlist.

---

## 4. UI/UX Design Specification

**Frontend:** **Svelte 5 + TypeScript + Vite**, styling CSS kustom (tanpa framework UI), ikon SVG inline.
**Alasan ukuran:** Svelte compile-time (tanpa runtime VDOM) menghasilkan bundle **150–250 KB** untuk ~12 layar; Vue 3 + router seperti Herd mendarat di ~1 MB+ dan React lebih besar. Untuk aplikasi yang asetnya **di-embed ke dalam EXE**, perbedaan ini langsung terasa di ukuran binary.
**Tanpa** source map di production, tanpa icon-font (pakai WOFF2 subset ≤ 20 KB bila perlu).

**Windows-native look:**
- Font: `Segoe UI Variable Text` / fallback `Segoe UI`.
- Token Fluent: radius 4–8 px, elevasi 1–2 layer, `Mica` **hanya** di window utama dan **wajib** `windowEffects` dengan catatan: blur/acrylic punya performa buruk saat resize/drag (catatan resmi Tauri) → default `Mica` OFF, opsi di Settings.
- Dark/light mengikuti `theme` OS + override manual; transisi tanpa flash (CSS `color-scheme`).
- Aksesibilitas: kontras WCAG AA, semua aksi keyboard-reachable, `focus-visible` ring 2 px.

**Window config:**

| Properti | Nilai |
|---|---|
| label | `main` |
| ukuran | 1080×720, `minWidth` 880, `minHeight` 560 |
| resizable | true |
| title | `Corral` |
| `center` | true |
| `decorations` | true |
| `backgroundColor` | `#f7f7f8` (light) — cegah white flash |
| `visible` | false → `show()` setelah first paint |
| `dragDropEnabled` | true (drop folder untuk `link`) |
| tray | `TrayIconBuilder` + menu: situs terakhir (max 8), Services, Restart All, Settings, Quit |
| single instance | plugin `single-instance` (klik kedua = fokus window) |

**Struktur navigasi (sidebar 220 px, 6 area):** Applications · Services · Mail · Dumps · Logs · Settings.

**ASCII wireframe — Applications (layar utama):**

```
┌───────────────────────────────────────────────────────────────────────────┐
│  Corral                                              [PHP 8.4 ▾] [⟳] [⚙]   │
├──────────────┬────────────────────────────────────────────────────────────┤
│ Applications │  Search sites…                    [+ New]  [Park folder]    │
│ Services     │  ┌──────────────────────────────────────────────────────┐  │
│ Mail      (3)│  │ ● my-app.test          PHP 8.4  HTTPS  Laravel 8.4  │  │
│ Dumps        │  │   ~/Herd/my-app           DB: my_app   [Open][⋯]     │  │
│ Logs         │  ├──────────────────────────────────────────────────────┤  │
│ Settings     │  │ ● coffee-tracker.test   PHP 8.2  HTTP   Laravel 8.2 │  │
│              │  │   ~/Herd/coffee-tracker   DB: coffee   [Open][⋯]     │  │
│              │  └──────────────────────────────────────────────────────┘  │
│ ── Status ── │  Workspaces:                                              │
│ nginx   ● up │   feature-dark-mode.my-app.test   PHP 8.4   [Open] [×]     │
│ php8.4  ● up │                                                            │
│ mariadb ● up │                                                            │
│ redis   ○ off│                                                            │
└──────────────┴────────────────────────────────────────────────────────────┘
```

**ASCII wireframe — Site detail (panel kanan / halaman):**

```
my-app.test                                            [Open site]  [Delete]
──────────────────────────────────────────────────────────────────────────────
Path          ~/Herd/my-app                    [Change]   [Open in Explorer]
Document root ~/Herd/my-app/public  (auto-detected: Laravel)
PHP version   ( ) 8.1  ( ) 8.2  ( ) 8.3  (•) 8.4  ( ) 8.5  ( ) 8.6
Node version  ( ) 20   (•) 22   ( ) 24
HTTPS         [● on]  CA: trusted ✓  expires 2027-08-11   [Renew] [Untrust]
Database      my_app          MySQL 11.4 (127.0.0.1:3306)   [Open Adminer]
Queue         [○ off] driver: database   Scheduler [● on]
Reverb        [○ off]  reverb.my-app.test
Env snippet   DB_HOST=127.0.0.1 DB_DATABASE=my_app MAIL_HOST=127.0.0.1 …
.corral.yml   detected ✓  (php: 8.4, workers: [queue])
```

**ASCII wireframe — Mail:**

```
Mail  (3 new)                                    [Clear all]  [Settings]
┌──────────────────────────┬─────────────────────────────────────────────┐
│ 12:04  Verify Email      │  To: dev@my-app.test                        │
│        → dev@my-app.test │  Subject: Verify Email Address              │
│ 11:58  Reset Password    │  ──────────────────────────────────────────  │
│        → admin@test      │  [ HTML preview iframe, sandboxed ]         │
│ 11:31  Order Shipped     │  Attachments: invoice.pdf (82 KB)           │
└──────────────────────────┴─────────────────────────────────────────────┘
```

**State UI:** setiap layar punya 3 state eksplisit — `loading` (skeleton, bukan spinner penuh layar), `empty` (copy actionable: "Belum ada situs. Tarik folder ke sini atau klik New."), `error` (pesan bahasa manusia + tombol Retry + link log). Tidak pernah layar kosong/putih saat error.

---

## 5. Technical Architecture — Clean Architecture

### 5.1 Cargo workspace (4 crate + 1 frontend)

```
corral/
├── Cargo.toml                     # [workspace]
├── crates/
│   ├── corral-core/               # domain + application (TANPA tauri, TANPA windows-ui)
│   │   └── src/{domain,application,ports}.rs
│   ├── corral-platform/           # infrastructure Windows (process, registry, hosts, certs, pipe)
│   │   └── src/{process,job,hosts,trust,pipe,port,defender,fs_store,sqlite,download}.rs
│   ├── corral-app/                # binary Tauri (presentation)
│   │   ├── src/{main.rs,lib.rs,commands/,state.rs,events.rs,channels.rs}
│   │   ├── capabilities/main.json
│   │   └── tauri.conf.json
│   ├── corral-helper/             # binary Windows service (privileged, minimal)
│   │   └── src/{main.rs,service.rs,pipe_server.rs,policy.rs}
│   ├── corral-cli/                # binary CLI `corral`
│   │   └── src/{main.rs,client.rs,commands/}
│   └── corral-mcp/                # binary MCP stdio server (P2)
├── ui/                            # Svelte 5 + TS + Vite → dist/ (di-embed)
│   ├── src/{routes,components,lib/ipc.ts,lib/errors.ts}
│   └── vite.config.ts
└── .github/workflows/release.yml
```

**Dependency rule (ditegakkan `cargo-deny`/review):**
`corral-app` → `corral-core` → (tidak ada). `corral-core` **tidak boleh** mengimpor `tauri`, `windows`, atau `tokio::process`; ia hanya bergantung pada **trait** (`ports.rs`). Implementasi nyata ada di `corral-platform`.

```rust
// crates/corral-core/src/ports.rs  — kontrak, bukan implementasi
pub trait ProcessSupervisorPort: Send + Sync {
    fn start(&self, spec: ServiceSpec) -> Result<ServiceId, CoreError>;
    fn stop(&self, id: &ServiceId) -> Result<(), CoreError>;
    fn status(&self, id: &ServiceId) -> ServiceStatus;
}
pub trait HostsPort: Send + Sync {
    fn replace_block(&self, entries: &[HostEntry]) -> Result<(), CoreError>;
}
pub trait TrustStorePort: Send + Sync {
    fn install_ca(&self, pem: &Path) -> Result<(), CoreError>;
    fn remove_ca(&self, thumbprint: &str) -> Result<(), CoreError>;
    fn is_trusted(&self, thumbprint: &str) -> Result<bool, CoreError>;
}
```

### 5.2 `lib.rs` / `main.rs` split (testable)

```rust
// crates/corral-app/src/lib.rs
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = app.get_webview_window("main").map(|w| w.set_focus());
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_log::Builder::default().build())
        .manage(AppStateHandle::new()?)          // type alias, lihat §8
        .setup(|app| { bootstrap(app)?; Ok(()) })
        .invoke_handler(tauri::generate_handler![
            commands::sites::list_sites, commands::sites::create_site,
            commands::php::switch_php,  commands::services::start_service,
            commands::tls::enable_https, commands::logs::tail_log,
            /* … total 68 command, lihat lampiran §17.4 */
        ]);
    builder.run(tauri::generate_context!())?;
    Ok(())
}

// crates/corral-app/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    install_panic_hook();                        // §9.3
    if let Err(e) = corral_app::run() {
        show_fatal_dialog(&e);                   // dialog, bukan silent death
    }
}
```

### 5.3 State & DI

```rust
pub struct AppServices {
    pub sites: Arc<SiteService>,
    pub php: Arc<PhpManager>,
    pub services: Arc<ServiceSupervisor>,
    pub nginx: Arc<NginxController>,
    pub tls: Arc<TlsService>,
    pub hosts: Arc<dyn HostsPort>,
    pub mail: Arc<MailStore>,
    pub db: Arc<Sqlite>,
}
pub type AppStateHandle = Mutex<AppServices>;   // alias WAJIB: State<'_ , Mutex<Wrong>> = panic runtime
```
- Semua service dibangun di `setup()` (bukan lazy global), disimpan via `app.manage`.
- **Tidak ada** `static mut`, **tidak ada** singleton global, **tidak ada** `lazy_static` untuk state yang bisa gagal init.
- Subscriptions (watcher, pipe server) memakai `CancellationToken` (`tokio_util`) agar bisa dimatikan saat restart komponen.

### 5.4 Alur request (runtime)

```
Browser → http://my-app.test
  │  (1) nama resolve: hosts file (blok Corral)  ← ditulis CorralHelper via named pipe
  ▼
(2) nginx.exe  listen 127.0.0.1:80 (dan :443 untuk HTTPS)
      server_name my-app.test www.my-app.test *.my-app.test
      root  C:/Users/<u>/Herd/my-app/public
  │  location ~ \.php  →  fastcgi_pass 127.0.0.1:9084   (base_php_port + 84)
  ▼
(3) php-cgi.exe (FastCGI, PHP_FCGI_CHILDREN=8) versi 8.4
      extensions: xdebug (opsional), redis, pdo_mysql, pgsql, mongodb
  ▼
(4) Laravel → MariaDB 127.0.0.1:3306, Redis 127.0.0.1:6379
             mail → Corral SMTP 127.0.0.1:1025 → SQLite → UI
             dump() → Corral Dumps 127.0.0.1:9912 → UI
             queue/scheduler/reverb → worker supervisor (per situs)
```

Kontrol dari UI: `invoke("switch_php", …)` → `SiteService` (core) → `NginxController` render template → `nginx -t` → atomic write → `nginx -s reload`. Setiap langkah mengembalikan status; kegagalan → rollback config lama.

### 5.5 Process supervision (Job Object — anti-zombie)

```rust
// crates/corral-platform/src/process/job.rs (ringkas)
pub struct JobHandle(windows::Win32::Foundation::HANDLE);
impl JobHandle {
    pub fn kill_on_close() -> Result<Self, PlatformError> {
        // CreateJobObjectW + SetInformationJobObject(
        //   JobObjectExtendedLimitInformation{ LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE })
    }
    pub fn assign(&self, child: &tokio::process::Child) -> Result<(), PlatformError>;
}
```
- Setiap `nginx.exe`, `php-cgi.exe`, `mariadbd.exe`, `redis-server.exe`, `php artisan …` di-`assign` ke satu job milik app.
- Penutupan app (normal/crash/kill dari Task Manager) → seluruh job mati. Tidak ada `php-cgi.exe` orphan (masalah paling sering di Laragon/XAMPP).
- Spawn dengan `CREATE_NO_WINDOW` (`windows::Win32::System::Threading::CREATE_NO_WINDOW`) supaya tidak ada jendela hitam berkedip.
- Graceful stop: kirim `nginx -s quit`, lalu `GenerateConsoleCtrlEvent`/`WM_CLOSE`, tunggu 3 s, baru `TerminateJobObject`.

---

## 6. Data Model

**Penyimpanan:**
- **SQLite** (`rusqlite` dengan feature `bundled`) di `%APPDATA%\Corral\corral.db` — untuk situs, workspace, mail, service instances, event log, ledger download. Alasan: query/filter mail & log perlu, dan `bundled` menghilangkan dependency DLL eksternal. Biaya: +~1,2 MB di EXE (dapat diterima; sudah masuk budget §7.6).
- **JSON** (`serde_json`, atomic write) untuk `settings.json` dan `manifest-cache.json`.
- **Tidak** memakai `tauri-plugin-store` untuk data terstruktur (tidak ada transaksi/indexing).

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: SiteId,                 // Uuid v7 (monotonic, sortable)
    pub name: String,               // "my-app"
    pub path: PathBuf,              // C:\Users\u\Herd\my-app
    pub document_root: Option<PathBuf>, // None = auto-detect
    pub hostnames: Vec<String>,     // ["my-app.test", "www.my-app.test", "*.my-app.test"]
    pub php_version: Option<String>,// None = ikut global
    pub node_version: Option<String>,
    pub https: bool,
    pub isolated_services: Vec<ServiceId>,
    pub workers: Vec<WorkerKind>,   // Queue | Scheduler | Reverb
    pub framework: Framework,       // Laravel | Plain | Unknown(osserved)
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInstance {
    pub id: ServiceId,
    pub kind: ServiceKind,          // Nginx | PhpCgi | MariaDb | Postgres | Redis | Mongo | Mail | Dumps | Worker
    pub version: String,
    pub port: u16,
    pub data_dir: Option<PathBuf>,
    pub pid: Option<u32>,
    pub state: ServiceState,        // Stopped | Starting | Running | Degraded | Failed{code}
    pub autostart: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Certificate {
    pub host: String,
    pub ca_thumbprint: String,
    pub leaf_path: PathBuf,
    pub key_path: PathBuf,          // DPAPI-protected, lihat §12
    pub not_after: i64,
    pub trusted: bool,
}
```

**Skema SQLite (ringkas):**

```sql
CREATE TABLE sites(id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL, path TEXT NOT NULL,
  document_root TEXT, php_version TEXT, node_version TEXT, https INTEGER NOT NULL DEFAULT 0,
  framework TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE hosts(site_id TEXT NOT NULL REFERENCES sites(id) ON DELETE CASCADE, host TEXT NOT NULL,
  PRIMARY KEY(site_id, host));
CREATE TABLE services(id TEXT PRIMARY KEY, kind TEXT NOT NULL, version TEXT NOT NULL,
  port INTEGER NOT NULL, data_dir TEXT, autostart INTEGER NOT NULL DEFAULT 1);
CREATE TABLE mails(id TEXT PRIMARY KEY, received_at INTEGER NOT NULL, envelope_from TEXT,
  subject TEXT, preview TEXT, raw BLOB NOT NULL);
CREATE TABLE mail_recipients(mail_id TEXT REFERENCES mails(id) ON DELETE CASCADE, address TEXT NOT NULL);
CREATE TABLE workspaces(id TEXT PRIMARY KEY, site_id TEXT NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  branch TEXT NOT NULL, path TEXT NOT NULL, host TEXT NOT NULL, db_name TEXT);
CREATE TABLE download_ledger(component TEXT, version TEXT, sha256 TEXT, installed_at INTEGER,
  PRIMARY KEY(component, version));
PRAGMA journal_mode = WAL;  PRAGMA synchronous = NORMAL;  PRAGMA foreign_keys = ON;
```

**DTO vs domain:** command **tidak** mengembalikan struct domain langsung. Ada `dto::SiteView` (subset, `camelCase`) di boundary; domain tetap internal. Tujuannya: (a) kontrol ukuran payload, (b) mencegah perubahan internal memecah API frontend.

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteView {
    pub id: String, pub name: String, pub host: String, pub path: String,
    pub php_version: String, pub https: bool, pub framework: &'static str,
    pub healthy: bool, pub db_name: Option<String>,
}
```

---

## 7. App Size Optimization

### 7.1 Cargo release profile (stable toolchain)

```toml
[profile.release]
codegen-units = 1
lto = true
opt-level = "s"      # "z" hanya nightly
panic = "abort"
strip = true
[profile.release.package."*"]
opt-level = "s"
```

### 7.2 Remove unused commands (Tauri ≥ 2.4)

```json
{ "$schema": "https://schema.tauri.app/config/2",
  "build": { "removeUnusedCommands": true } }
```
Syarat: `tauri@>=2.4`, `tauri-build@>=2.1`, `tauri-plugin@>=2.1`, `tauri-cli@>=2.4`, dan ACL dipakai benar (command yang tidak ada di capability akan dikompilasi keluar). Konsekuensi: 68 command di §17.4 **harus** tercantum lengkap di `capabilities/main.json`, kalau tidak akan "hilang" di release.

### 7.3 Disiplin dependency

- `tauri = { version = "2", default-features = false, features = ["wry", "common-controls-v6", "tray-icon", "image-png"] }` — buang `protocol-asset`/`devtools` di release.
- Plugin hanya yang dipakai: `single-instance`, `window-state`, `log`, `opener`, `notification`, `autostart`, `updater`, `clipboard-manager`, `os`. **Tidak** memakai `shell` plugin (semua spawn dari Rust → surface lebih kecil + ukuran lebih kecil).
- `tokio = { features = ["rt-multi-thread","process","net","time","fs","sync","io-util","macros"] }` (tanpa `full`).
- `reqwest` **hanya** untuk download runtime: `default-features = false, features = ["rustls-tls-native-roots","http2","stream"]` (hindari OpenSSL).
- Dev-only: `#[cfg(debug_assertions)]` untuk mock engine & devtools.
- CI menegakkan `cargo bloat --release --crates -n 30` disimpan sebagai artefak; regresi > 200 KB per crate gagal.

### 7.4 Frontend diet (aset di-embed ke EXE)

- Svelte 5, `vite build` + `terser`, `cssCodeSplit: false`, **tanpa** source map.
- Tanpa icon-font; SVG di-inline via komponen (tree-shaken).
- Kompresi aset gambar ke WebP; splash/OG image tidak dibundel.
- Target `dist/`: ≤ 250 KB (gzip ≤ 90 KB). Anggaran per route ≤ 25 KB.
- Ikon tray 32×32 PNG ≤ 3 KB (ikon besar = EXE besar; catatan resmi Tauri).

### 7.5 Installer Windows

```json
{ "bundle": {
    "targets": ["nsis"],
    "windows": {
      "webviewInstallMode": { "type": "downloadBootstrapper" },
      "nsis": { "installMode": "currentUser", "compression": "lzma",
                "displayLanguageSelector": false },
      "staticVCRuntime": true
    },
    "createUpdaterArtifacts": true
} }
```

| WebView2 mode | Butuh internet | Delta installer |
|---|---|---|
| `downloadBootstrapper` (dipilih) | Ya | 0 MB |
| `embedBootstrapper` | Ya | +1,8 MB |
| `offlineInstaller` | Tidak | +127 MB |
| `fixedVersion` | Tidak | +180 MB |
| `skip` | — | 0 MB (tidak disarankan) |

**Strategi runtime on-demand (kunci ukuran):** nginx (~4,5 MB) **dibundel**; PHP/database/Redis **tidak** dibundel — diunduh per versi dari CDN kita, diverifikasi `sha256` + tanda tangan **Ed25519** di manifest, lalu di-cache di `%LOCALAPPDATA%\Corral\runtimes`. Instalasi pertama butuh internet; UI menampilkan progress per komponen dengan `Channel`.

### 7.6 Size budget

| Artefak | Target | Batas keras |
|---|---|---|
| `corral.exe` (release, stripped) | < 5,5 MB | 6 MB |
| `corral-helper.exe` | < 1,2 MB | 1,5 MB |
| `corral.exe` CLI + MCP (masing-masing) | < 1,5 MB | 2 MB |
| Bundle frontend `dist/` | < 250 KB | 300 KB |
| Installer NSIS | < 7,5 MB | 8 MB |
| RAM idle (Rust + WebView2 terpisah) | < 90 MB | 120 MB |

---

## 8. Memory Management & Efficiency

**Aturan command:**
1. Command yang menyentuh disk/network/proses/CPU loop **wajib** `async fn` (jalan di `async_runtime`, bukan main thread).
2. Command sinkron hanya untuk baca state di memori (`get_settings`, `list_sites_cached`).
3. Command async **tidak boleh** menerima `&str` atau `State<'_, T>`(limitation resmi) → konversi ke tipe owned di signature.

```rust
#[tauri::command]
async fn start_site_worker(
    state: tauri::State<'_, AppStateHandle>,   // OK: State boleh, yang dilarang adalah referensi
    site_id: String,                           // owned, bukan &str
    kind: WorkerKindDto,
) -> Result<WorkerStatus, AppError> {
    let svc = { state.lock().map_err(|_| AppError::StateLock)?.sites.clone() };
    let site = svc.get(&site_id)?;                    // validasi ada
    svc.start_worker(&site, kind.into()).map_err(AppError::from)  // tanpa unwrap
}
```

**Streaming (wajib untuk log, progress download, output artisan):**

```rust
#[tauri::command]
async fn tail_log(
    path: String,
    on_line: tauri::ipc::Channel<LogLine>,
) -> Result<(), AppError> {
    // ring buffer 2 MB; flush per 50 ms atau 200 baris, mana dulu
    for line in LogTail::open(&path)?.lines_streamed() {
        on_line.send(line)?;                          // ordered, tanpa JSON per-event
    }
    Ok(())
}
```
- Data biner (gambar attachment mail, file besar) → `tauri::ipc::Response::new(bytes)` (lewati JSON).
- Event system (`emit_to`) **hanya** untuk sinyal kecil & jarang: `service-state-changed`, `site-updated`, `update-available`.

**State & clone:**
- Satu `Mutex<AppServices>`; lock dipegang **hanya** untuk mengambil `Arc` (clone pointer), tidak selama I/O.
- Struct besar (config nginx hasil render, mail body) tidak disimpan di state; tulis ke disk lalu kirim path/handle.
- Frontend: list situs/mail **virtualized** (windowing manual, bukan render 5.000 node); setiap `listen()` wajib punya pasangan `unlisten()` di `onDestroy`.

**Budget memori per fitur:**

| Fitur | Budget |
|---|---|
| Ring buffer log per tab | 2 MB |
| Cache preview mail (20 terakhir) | 8 MB |
| Parser nginx config / template | < 1 MB sekali render |
| Index pencarian log 10 MB | ≤ 6 MB |
| Payload IPC per call | ≤ 2 MB (di atas itu → Channel/Response) |
| List 1.000 situs (DOM) | ≤ 40 MB WebView2 |

---

## 9. Error Handling — Zero Crash Policy

### 9.1 Error domain (`thiserror`), dipakai di seluruh lapisan

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("kesalahan I/O: {0}")]                 Io(#[from] std::io::Error),
    #[error("input tidak valid: {0}")]             Validation(String),
    #[error("situs tidak ditemukan: {0}")]          SiteNotFound(String),
    #[error("port {port} dipakai proses {process} (PID {pid})")] PortInUse { port: u16, pid: u32, process: String },
    #[error("konfigurasi nginx tidak valid: {0}")] NginxConfig(String),
    #[error("helper belum terpasang")]             HelperMissing,
    #[error("helper menolak permintaan: {0}")]      HelperRejected(String),
    #[error("komponen {component} {version} gagal diunduh: {reason}")] Download { component: String, version: String, reason: String },
    #[error("sertifikat kedaluwarsa: {0}")]         CertExpired(String),
    #[error("state terkunci")]                     StateLock,
    #[error("butuh hak administrator: {0}")]        ElevationRequired(String),
}
```

### 9.2 Serialisasi terstruktur ke frontend

```rust
#[derive(Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum ErrorKind {
    Io(String), Validation(String), SiteNotFound(String), PortInUse(String),
    NginxConfig(String), HelperMissing(String), HelperRejected(String),
    Download(String), CertExpired(String), StateLock(String), ElevationRequired(String),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let m = self.to_string();
        let k = match self {
            Self::Io(_) => ErrorKind::Io(m),           Self::Validation(_) => ErrorKind::Validation(m),
            Self::SiteNotFound(_) => ErrorKind::SiteNotFound(m),
            Self::PortInUse { .. } => ErrorKind::PortInUse(m),
            Self::NginxConfig(_) => ErrorKind::NginxConfig(m),
            Self::HelperMissing => ErrorKind::HelperMissing(m),
            Self::HelperRejected(_) => ErrorKind::HelperRejected(m),
            Self::Download { .. } => ErrorKind::Download(m),
            Self::CertExpired(_) => ErrorKind::CertExpired(m),
            Self::StateLock => ErrorKind::StateLock(m),
            Self::ElevationRequired(_) => ErrorKind::ElevationRequired(m),
        };
        k.serialize(s)
    }
}
```

**Mirror TypeScript** (`ui/src/lib/errors.ts`):

```ts
export type AppErrorKind =
  | { kind: 'io'; message: string }
  | { kind: 'validation'; message: string }
  | { kind: 'siteNotFound'; message: string }
  | { kind: 'portInUse'; message: string }
  | { kind: 'nginxConfig'; message: string }
  | { kind: 'helperMissing'; message: string }
  | { kind: 'helperRejected'; message: string }
  | { kind: 'download'; message: string }
  | { kind: 'certExpired'; message: string }
  | { kind: 'stateLock'; message: string }
  | { kind: 'elevationRequired'; message: string };

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    const err = parseAppError(e);            // normalisasi string/objek
    toast.error(messageFor(err));            // bahasa manusia, bukan stack trace
    throw err;
  }
}
window.addEventListener('unhandledrejection', ev => { report(ev.reason); });
```

### 9.3 Panic hook global

```rust
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = tracing::error!(target: "panic", "{info}");
        persist_crash_report(info);             // %APPDATA%\Corral\crash\<ts>.txt
        show_fatal_dialog(info);                // MessageBoxW / tauri dialog
        default(info);
    }));
}
```

### 9.4 Aturan keras
- `unwrap()`, `expect()`, `panic!()`, `unreachable!()`, slicing index tanpa cek → **dilarang** di jalur produksi. Diizinkan hanya di `main()` bootstrap, test, dan init statis dengan komentar `// SAFETY: …`.
- Ditegakkan CI: `RUSTFLAGS="-D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::indexing_slicing"` untuk crate non-test.
- Semua `Result` dipropagasi `?`; error tidak pernah ditelan (`let _ =`) kecuali logging eksplisit untuk operasi best-effort (didaftarkan di allowlist `best_effort` dengan komentar alasan).
- Setiap error punya: pesan manusia + `tracing` field (`site_id`, `port`, `component`) + tindakan lanjutan yang bisa dilakukan user.
- Helper service: **tidak pernah** panic saat menolak request; selalu balas `{"ok":false,"code":"..."}` dan log.

---

## 10. Memaksimalkan Tauri 2 API

**Capability file** (`crates/corral-app/capabilities/main.json`) — command ditolak by default di v2:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "main-capability",
  "description": "Corral main window: hanya command yang terdaftar",
  "windows": ["main"],
  "permissions": [
    "core:default", "core:window:allow-start-dragging", "core:window:allow-minimize",
    "core:webview:allow-internal-toggle-devtools",
    "opener:default", "notification:default", "clipboard-manager:allow-write-text",
    "os:allow-platform", "updater:default", "autostart:allow-enable", "autostart:allow-disable",
    "log:default",
    "allow-list-sites", "allow-create-site", "allow-switch-php", "allow-start-service",
    "allow-stop-service", "allow-enable-https", "allow-tail-log", "allow-open-path"
  ]
}
```
>`devtools` tidak diaktifkan di release (`core:webview:allow-internal-toggle-devtools` hanya untuk window dev).

**Inventaris plugin (versi dipatok sama dengan crate):**

| Plugin | Versi | Dipakai untuk | Catatan size |
|---|---|---|---|
| `single-instance` | 2.x | fokus window saat klik kedua | kecil |
| `window-state` | 2.x | ingat geometri & maximize | kecil |
| `log` | 2.x | log terstruktur → file, level release = `warn` | kecil |
| `opener` | 2.x | buka URL/folder di browser/Explorer | kecil |
| `notification` | 2.x | notifikasi native (service mati, cert expired) | kecil |
| `autostart` | 2.x | start on login | kecil |
| `updater` | 2.x | auto-update + signature | +~200 KB |
| `clipboard-manager` | 2.x | copy env snippet / host | kecil |
| `os` | 2.x | info platform/versi | kecil |
| **tidak dipakai** | — | `shell`, `fs`, `http`, `sql` | spawn & I/O dilakukan di Rust → surface & ukuran lebih kecil |

**API v2 lain yang wajib dipakai (bukan v1):**
- `WebviewWindow` (bukan `Window`), `TrayIconBuilder` (bukan `SystemTray`), `tauri::ipc::Channel`, `ipc::Response`.
- `#[tauri::command(async)]` / `async fn` untuk kerja berat.
- Window effects: `windowEffects: { effects: ["mica"] }` — **warning**: blur/acrylic memberi beban GPU saat resize/drag (catatan resmi) → default OFF, ditawarkan di Settings.
- `scrollBarStyle: "fluentOverlay"` (WebView2 125+), fallback otomatis bila tidak didukung.
- `emit_to(EventTarget::webview_window("main"), …)` untuk event terarah, bukan broadcast global.
- `contentProtected` OFF (kita tidak butuh anti screen capture), `devtools` OFF di release.

---

## 11. Windows x64 Build & Distribution

- **Target:** `x86_64-pc-windows-msvc` (single arch). MSRV Rust ≥ 1.82; Tauri 2.11.x; WebView2 runtime terpasang (Win10 1809+/Win11 sudah ada).
- **Build:** `npm run tauri build` (host Windows), atau CI `windows-latest` dengan `tauri-action`.
- **NSIS:** `installMode: currentUser` (tanpa admin), `compression: lzma`, `staticVCRuntime: true`.
- **Code signing:** `certificateThumbprint` + `digestAlgorithm: "sha256"` + `timestampUrl` (RFC 3161) + `tsp: true`. Tanpa signing → SmartScreen warning (dokumentasikan di halaman unduh).
- **Helper service:** dipasang **setelah** install lewat UAC sekali:
  `sc.exe create CorralHelper binPath= "…\corral-helper.exe" start= auto obj= LocalSystem` + `sc.exe start`. Uninstaller wajib `sc.exe stop` + `sc.exe delete` dan menghapus blok hosts + CA (via helper sebelum dihapus).
- **First-run flow:** health check WebView2 → tawarkan pasang helper (UAC) → unduh nginx (bundled) & PHP default 8.4 → generate CA → trust (UAC bila perlu) → tulis hosts → start nginx+php → tampilkan onboarding 3 langkah.
- **Distribusi runtime component:** `https://cdn.<domain>/corral/manifest.json` (Ed25519-signed) berisi `{component, version, url, sha256, sia, size}`; client memverifikasi signature **sebelum** unduh & sha256 sesudah unduh, ekstrak ke folder versi (tidak menimpa), lalu update `download_ledger`.
- **Updater:** `createUpdaterArtifacts: true`, endpoint `https://cdn.<domain>/corral/update/{{target}}/{{arch}}/{{current_version}}`, channel stable/beta, `health_check` pasca-update (app harus bisa start & health endpoint menjawab ≤ 30 s, kalau tidak → rollback ke versi sebelumnya yang masih di `%LOCALAPPDATA%\Corral\prev`).

---

## 12. Security

- **Helper = permukaan serangan utama.** Named pipe `\\.\pipe\corral-helper-v1` dengan security descriptor eksplisit: hanya SID user pemilik instalasi (bukan `Everyone`, bukan `Authenticated Users`); mode `PIPE_REJECT_REMOTE_CLIENTS`.
- **Validasi di sisi helper, bukan pemanggil:**
  - hosts: tolak payload berisi IP literal; TLD allowlist `{"test","local","wip","corral"}`; max 500 entri; max 64 KB; karakter `[a-z0-9.-]` saja.
  - cert: path harus di dalam `%APPDATA%\Corral\ca\` (canonicalize + prefix check, cegah traversal); hanya `.pem`/`.crt`.
  - firewall/Defender: path harus di dalam folder instalasi Corral (canonicalize).
  - setiap request membawa `{nonce, hmac}` dengan kunci yang dibagikan saat install (disimpan DPAPI `CryptProtectData`, bukan plaintext di file).
- **Rahasia & kunci privat:** private key CA & leaf disimpan dengan DPAPI (`CryptProtectData` per-user). Tidak pernah dikirim ke webview. Frontend hanya menerima path/thumbprint/expiry.
- **Webview:** CSP ketat `default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-src 'none'; connect-src ipc: http://ipc.localhost`; `freezePrototype: true`; tanpa `--disable-web-security`; tidak ada remote URL di `devUrl`/production.
- **Preview HTML mail** dirender di `<iframe sandbox="allow-same-origin">` dengan `srcdoc` yang sudah di-sanitize (script diblok, `on*` dihapus, `http(s)` eksternal dijadikan placeholder) — tidak pernah di-load dari URL asli.
- **Download runtime:** hanya dari host yang di-pin; TLS wajib; signature Ed25519 diverifikasi sebelum eksekusi; manifest di-cache dengan TTL 24 jam + `ETag`.
- **Audit:** `cargo audit` + `cargo deny` di CI (licenses: hanya MIT/Apache-2.0/BSD/ISC/ISC-like; menolak GPL/AGPL karena distribusi binary).
- **Least privilege:** app berjalan sebagai user biasa; hanya helper yang `LocalSystem`, dan helper **tidak** punya kemampuan spawn proses arbitrer (tidak ada shell exec di helper).

---

## 13. Testing Strategy

| Lapis | Jenis | Cakupan target | Alat |
|---|---|---|---|
| `corral-core` | unit (pure) | domain (deteksi Laravel/doc root, naming host, template context), state machine service, policy restart | `cargo test`, tanpa I/O |
| `corral-platform` | integration (butuh Windows) | hosts block writer (round-trip + rollback), Job Object kill, port table, rcgen chain verify, ini/env edit | `cargo test --features integration` |
| Helper | security tests | 200 payload fuzz (injection, traversal, IP literal, TLD ilegal, HMAC salah) → semua ditolak & log | `proptest` + corpus |
| `corral-app` | integration | `tauri::test::mock_builder` + invoke command: sukses & setiap varian `AppError` | `tauri = { features = ["test"] }` |
| Frontend | unit + komponen | `ipc.ts` mapping error, form validation, virtualized list | `vitest` + `@testing-library/svelte` |
| E2E | manual checklist | tabel QA di bawah | dokumen QA |

**QA matrix (wajib lulus sebelum rilis):**

1. Win11 x64 fresh VM: install → onboarding → buat situs `laravel new` → `http://x.test` tampil welcome page.
2. Win10 1809 x64: WebView2 belum ada → bootstrapper jalan, app tetap start.
3. Tanpa hak admin: app start, situs via `127.0.0.1:8080` berfungsi, banner helper muncul, tidak ada crash.
4. Port 80 dipakai IIS/Laragon → pesan menyebut PID & nama proses, opsi ganti port berhasil.
5. Dua situs PHP 8.2 & 8.5 simultan → keduanya benar.
6. HTTPS on/off + untrust CA → tidak ada warning setelah trust, warning kembali setelah untrust.
7. Kill app via Task Manager → verifikasi di Process Explorer: 0 `nginx.exe`/`php-cgi.exe`/`mysqld.exe` yatim.
8. Mail: 3 email masuk → tampil, attachment bisa dibuka, clear all berfungsi.
9. Workspace: buat 3 worktree → isolasi DB terbukti (insert di satu tidak terlihat di lain).
10. Update path: versi N → N+1, situs tetap jalan; simulasi update rusak → rollback berhasil.
11. Uninstall: service dihapus, CA dicabut dari store, blok hosts dibersihkan, datadir ditawarkan untuk dihapus.
12. 8 jam idle: RAM app < 120 MB, tidak ada handle leak (`handle -p corral.exe`).

---

## 14. CI/CD Pipeline

`/.github/workflows/release.yml` — runner `windows-latest`:

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::indexing_slicing` (crate non-test)
3. `cargo test --workspace` (unit) + `cargo test --workspace --features integration` (Windows-only)
4. `cargo audit` + `cargo deny check licenses`
5. `npm ci && npm run lint && npm run test -- --run && npm run build` (cek ukuran `dist/` ≤ 300 KB, gagal bila lebih)
6. `cargo bloat --release --crates -n 30` → simpan artefak
7. `tauri-apps/tauri-action@v0` → build NSIS + updater artifacts, sign updater, upload ke release
8. `signtool` untuk `corral.exe`, `corral-helper.exe`, `corral.exe` CLI, `corral-mcp.exe`
9. Gate ukuran: `corral.exe ≤ 6 MB`, `installer ≤ 8 MB` → gagal build bila lewat
10. Smoke test headless: install di runner, jalankan `corral doctor --json`, cek exit 0

Release channel: tag `vX.Y.Z` → stable; branch `beta` → prerelease; `createUpdaterArtifacts` wajib agar updater punya `latest.json`.

---

## 15. Performance & Size Budget

| Metrik | Target | Cara ukur |
|---|---|---|
| `corral.exe` | < 5,5 MB | CI gate (langkah 9) |
| Installer NSIS | < 7,5 MB | CI gate |
| Bundle frontend | < 250 KB | `du -b ui/dist` |
| Cold start (proses → window first paint) | < 1,2 s | `cmd_start` marker + tracing span |
| Waktu start nginx+php setelah app start | < 900 ms | log timestamp |
| Ganti versi PHP per situs | < 400 ms | benchmark command |
| Enable HTTPS (generate+trust) | < 1,5 s (trust butuh UAC) | benchmark |
| Resolusi situs baru (tulis hosts + reload) | < 700 ms | benchmark |
| Memory idle (Rust) | < 60 MB | `Get-Process` Private Bytes |
| Memory idle (total + WebView2) | < 120 MB | idem |
| IPC command latency lokal | < 5 ms (p95) | `tracing` histogram |
| Streaming log | 5 MB/s tanpa drop, ring buffer 2 MB | stress test |
| SQLite query list 1.000 situs | < 10 ms | `EXPLAIN QUERY PLAN` + bench |
| Error rate command | 0 panic / 10.000 invoke fuzz | fuzz test |

---

## 16. Risks & Mitigations

| Risiko | Dampak | Mitigasi |
|---|---|---|
| nginx Windows hanya single worker, ~1024 connection, tanpa epoll | performa jauh di bawah Linux; salah frame bisa dianggap "lambat" | dokumentasikan eksplisit; tuning `worker_connections 1024`; sediakan mode proxy ke container bila user butuh beban besar |
| PHP resmi Windows tidak punya php-fpm | harus pakai `php-cgi` FastCGI | sudah jadi desain inti (port base+versi); pantau stabilitas; kesehatan dicek per versi |
| Defender/SmartScreen memblok proses & binary hasil unduhan | app "tidak jalan" tanpa error jelas | `Add-MpPreference -ExclusionPath` via helper (opt-in, dengan penjelasan), signing code, checksum & signature verify |
| Hyper-V/WSL reservasi port 80/443/3306 | service gagal start | deteksi `excludedportrange` + diagnosa istimewa + opsi port alternatif |
| Helper service tidak terpasang (korporat, tanpa admin) | domain `.test` tidak otomatis | degraded mode (DNS-4) — app tetap berguna, banner jelas |
| Antivirus menganggap helper (LocalSystem + pipe + hosts write) sebagai PUP | false positive | signing, minimalkan kemampuan helper, publikasikan sumber/deskripsi, submit ke vendor AV |
| Lisensi redistribusi runtime (PHP, MariaDB GPL, Redis) | masalah legal | **tidak** membundel GPL binary di installer; unduh on-demand dari sumber resmi user; catat lisensi di UI "About → Licenses"; pertimbangkan Valkey (BSD) untuk Redis |
| WebView2 hilang/kedaluwarsa di Win10 lama | app tidak render | `downloadBootstrapper` + `minimumWebview2Version` + dialog penjelasan |
| Update merusak instalasi | app mati setelah update | health check + rollback binary (simpan 1 versi sebelumnya) |
| Konflik dengan Herd/Laragon terpasang bersamaan | port & hosts bentrok | deteksi produk lain saat onboarding, tawarkan matikan atau beda port; hosts block diberi marker sendiri |
| `removeUnusedCommands` menghapus command yang lupa didaftarkan | fitur hilang diam-diam di release | test integrasi `invoke` semua 68 command ×2 (debug+release) di CI |

---

## 17. Appendix

### 17.1 Pemetaan hasil reverse engineering Herd 2.1.1 → keputusan Corral

| Temuan RE (teramati) | Bukti | Keputusan Corral |
|---|---|---|
| Installer NSIS 3 Unicode, payload `app-64.7z` 466 MB | 7-Zip: 15 entri, header 143.204 B | NSIS (Tauri) + runtime on-demand → installer < 8 MB |
| Electron 3 proses; renderer Vue (13 entry HTML); preload 357 channel IPC | `app.asar` 156 MB, 19.521 entri | Svelte 5 + 68 command Rust bertipe |
| Main process = bytecode V8 (`index.jsc` 2,5 MB + `bytecode-loader.cjs`) | `out/main/index.js` 3 baris | Rust native (tanpa JIT/bytecode), logika terbaca & testable |
| `HerdHelper.exe` = .NET 8 service, `HttpListener`, `GET /status`, `POST /updateHosts`, TLD allowlist `test/local/wip/herd`, tolak IP literal, manipulasi ACL hosts | REA `inspect_managed_members` (90 user string) | Helper Rust + **named pipe** (bukan HTTP/port), validasi sama + HMAC |
| PHP: `php-cgi.exe` FastCGI, port = `base + versiInteger`, `$herd_sock_<ver>` | `Valet/PhpFpm.php` | Diadopsi identik (dipaksa oleh platform) |
| nginx `default_server 127.0.0.1:80`, `fastcgi_pass $herd_sock`, `client_max_body_size 512M` | `config/nginx/herd.conf` | Template setara + validasi `nginx -t` + rollback |
| CA + leaf 396 hari via phpseclib, trust `certutil -addstore` | `Valet/Site.php`, `bin/trust-cert.ps1` | `rcgen` + CryptoAPI (tanpa PHP, tanpa shell) |
| `dnsmasq.conf` (`address=/.test/127.0.0.1`) ada tapi jalur Windows = hosts + `HostsWatcher` | `config/dnsmasq/`, string main | hosts-first (sama); DNS server lokal = P3 |
| Service: `nginx.exe`, `php-cgi.exe`, `mariadbd/mysqld`, `postgres.exe`, `redis-server.exe`, Reverb, mail catcher (`smtp-server`) | string main + `resources/bin` | Supervisor + Job Object, mail catcher Rust |
| Fitur Pro: Studio AI widget inject, devtools helper, opencode, whisper | `resources/bin/*.zip` | **out of scope v1** (lihat §3.9) |
| `herd-mcp.phar` + channel `mcp.install` | preload channel list | `corral-mcp` (Rust, stdio) di P2 |
| Telemetri: Sentry, Aptabase, `node-machine-id`, `licensing.*` | `package.json`, channel list | **opt-in** crash report saja; tanpa analytics default |

### 17.2 Versi crate (pin eksak di `Cargo.lock`)

`tauri 2.11.x` · `tauri-build 2.x` · `tauri-plugin-{single-instance,window-state,log,opener,notification,autostart,updater,clipboard-manager,os} 2.x` · `tokio 1.x` · `serde/serde_json 1.x` · `thiserror 2.x` · `rusqlite 0.3x (bundled)` · `rcgen 0.13` · `rustls 0.23` · `reqwest 0.12 (rustls)` · `sha2 0.10` · `ed25519-dalek 2.x` · `notify 8.x` · `minijinja 2.x` · `zip 2.x` · `mail-parser 0.10` · `windows 0.5x` · `windows-service 0.7` · `tokio-util 0.7` · `tracing 0.1` · `uuid 1.x (v7)` · `time 0.3` · `semver 1`
Frontend: `svelte 5`, `vite 6`, `typescript 5`, `vitest 3`, `@tauri-apps/api 2.x` (**minor harus sama dengan crate `tauri`**).

### 17.3 Struktur direktori runtime (di mesin user)

```
%APPDATA%\Corral\            settings.json, corral.db, etc\nginx\{nginx.conf,sites\*.conf,server_snippets}
                             logs\{nginx-error.log,php-cgi-<ver>.log,corral.log}, ca\{ca.pem,ca.key(DPAPI),sites\*.pem}
%LOCALAPPDATA%\Corral\       runtimes\{php\8.4\,nginx\,mariadb\,redis\}, cache\downloads\, prev\ (rollback)
C:\ProgramData\Corral\       helper\service.json (port/pipe name, nonce key id), helper\logs\
C:\Users\<u>\Herd\           lokasi default situs (dapat diubah)
```
Alasan pemisahan: `%APPDATA%` (roaming config kecil) vs `%LOCALAPPDATA%` (runtime besar, tidak ikut roaming) vs `%ProgramData%` (dibaca service `LocalSystem`).

### 17.4 Permukaan IPC (68 command, dikelompokkan)

`app` (6): `app_info`, `open_path`, `open_url`, `set_theme`, `autostart_get/set`
`sites` (12): `list_sites`, `get_site`, `create_site`, `delete_site`, `link_folder`, `park_folder`, `unpark_folder`, `detect_framework`, `set_doc_root`, `set_domain`, `open_in_browser`, `open_in_editor`
`php` (8): `list_php_versions`, `install_php`, `uninstall_php`, `switch_php_global`, `switch_php_site`, `get_php_ini`, `set_php_ini`, `list_php_extensions`
`node` (4): `list_node_versions`, `install_node`, `switch_node_site`, `install_composer`
`services` (11): `list_services`, `start_service`, `stop_service`, `restart_service`, `start_all`, `stop_all`, `service_status`, `set_autostart`, `create_extra_service`, `delete_extra_service`, `service_logs`
`db` (6): `list_databases`, `create_database`, `drop_database`, `open_adminer`, `db_credentials`, `run_readonly_query`
`tls` (6): `generate_ca`, `trust_ca`, `untrust_ca`, `enable_https`, `disable_https`, `cert_status`
`logs` (4): `tail_log`, `list_log_files`, `clear_log`, `export_log`
`mail` (5): `list_mails`, `get_mail`, `delete_mail`, `clear_mails`, `mail_settings`
`dumps` (2): `list_dumps`, `clear_dumps`
`workspace` (4): `list_workspaces`, `create_workspace`, `delete_workspace`, `apply_workspace_config`
`system` (4): `doctor`, `helper_status`, `install_helper`, `port_report`
Total = **68** (semua wajib terdaftar di capability, lihat §7.2).

### 17.5 Roadmap implementasi

| Milestone | Isi | Estimasi |
|---|---|---|
| M0 — skeleton | workspace cargo, Tauri shell, capability, error contract, UI shell + IPC typed | 1 minggu |
| M1 — lahir | supervisor + Job Object, nginx bundle, PHP 8.4 FastCGI, situs `.test` via hosts (helper v0), HTTPS CA | 2–3 minggu |
| M2 — usable | multi PHP, situs settings, DB MariaDB, mail catcher, log viewer, CLI `corral` dasar | 2–3 minggu |
| M3 — DX | Node/Composer, xdebug toggle, dumps, workers (queue/scheduler/reverb), `.corral.yml`, doctor | 2 minggu |
| M4 — plus | workspace, Postgres/Redis/Mongo, updater+signing, installer polish, dokumentasi | 2–3 minggu |
| M5 — ekosistem | `corral-mcp`, reverse proxy situs, auto-renew TLS, telemetri opt-in | backlog |

### 17.6 Glosarium

- **FastCGI port**: `php-cgi.exe` sebagai server FastCGI di TCP; nginx memanggilnya per situs.
- **Job Object**: objek kernel Windows pengikat proses; `KILL_ON_JOB_CLOSE` menjamin anak mati saat app berhenti.
- **Helper**: service `LocalSystem` kecil untuk operasi privileged (hosts, cert store, firewall, Defender).
- **Degraded mode**: mode tanpa helper; fungsionalitas inti tetap, domain otomatis mati.
- **DPAPI**: `CryptProtectData`, enkripsi per-user untuk kunci privat.

---

*Dokumen ini adalah spesifikasi desain, bukan hasil observasi runtime Corral. Fakta "teramati" pada §17.1 berasal dari analisis statis artefak Herd 2.1.1 (Windows x64) dan tidak menyatakan perilaku runtime Herd yang tidak diuji.*
