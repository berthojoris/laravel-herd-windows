# Corral — Laravel & PHP Dev Server untuk Windows x64 (Rust + Tauri 2 + Svelte 5)

**Corral** adalah aplikasi desktop native Windows x64 (`x86_64-pc-windows-msvc`) yang menyediakan environment pengembangan **PHP & Laravel** kelas produksi setara **Laravel Herd for Windows** tanpa Docker dan tanpa WSL.

---

## Fitur Utama

1. **Laravel App Server Optimizer (Khusus Windows x64):**
   - **Zend OPcache + PHP 8 Tracing JIT (`128M`):** Memangkas waktu boot container Laravel dan autoload Composer hingga 70%.
   - **Windows NTFS Realpath Stat Cache (`4096K`, TTL `600s`):** Menghilangkan bottleneck operasi `stat()` ribuan file di folder `vendor/` pada filesystem NTFS.
   - **Nginx FastCGI Buffer & Pool Tuning:** Konfigurasi `fastcgi_buffers 16 32k`, `fastcgi_buffer_size 64k`, `client_max_body_size 512M`, dan `PHP_FCGI_CHILDREN=8`.
   - **Sinkronisasi `.env` Non-Destruktif 1-Klik:** Mengisi `APP_URL`, `DB_*`, `REDIS_*`, `MAIL_PORT=1025`, dan `VAR_DUMPER_SERVER=127.0.0.1:9912` tanpa menghapus komentar asli.
   - **Integrated Artisan & Worker Runner:** Eksekusi `optimize:clear`, `migrate`, `storage:link`, `route:list`, `about`, serta supervisi background worker (`queue:work`, `schedule:work`, `reverb:start`) dengan *auto-restart backoff* (maksimal 3× dalam 60 detik).

2. **Anti-Zombie Process Supervision (Windows Job Object):**
   - Seluruh proses anak (`nginx.exe`, `php-cgi.exe`, `mariadbd.exe`, `redis-server.exe`, `php artisan`) diikat ke kernel **Windows Job Object** dengan flag `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` dan `CREATE_NO_WINDOW`.
   - Menutup aplikasi atau mematikan proses dari Task Manager menjamin **0 proses yatim (zombie)**.

3. **System Tray & Minimize-to-Tray Kelas Produksi:**
   - Mendukung **Close to System Tray** dan **Minimize to System Tray** sehingga server Nginx, PHP FastCGI, dan MariaDB tetap melayani domain `.test` di background tanpa memenuhi Taskbar.
   - Klik kiri pada ikon System Tray langsung memulihkan dan memfokuskan jendela utama Corral; klik kanan membuka menu cepat (*Open Dashboard*, *Start/Restart/Stop All Services*, *Quit Corral*).

4. **Keamanan Privileged Helper (`CorralHelper`):**
   - Komunikasi via **Windows Named Pipe** (`\\.\pipe\corral-helper-v1`) dengan autentikasi **HMAC-SHA256 + Nonce**.
   - Validasi ketat di sisi helper: menolak IP literal, membatasi TLD ke allowlist (`.test`, `.local`, `.wip`, `.corral`), dan mencegah *path traversal*.
   - **Degraded Mode (Non-Admin):** Tetap berjalan penuh tanpa crash pada laptop tanpa hak Administrator.

---

## Struktur Workspace Cargo (Clean Architecture)

```text
corral/
├── Cargo.toml                     # Workspace root + release size optimization + strict clippy lints
├── crates/
│   ├── corral-core/               # Pure Domain, Application, Ports (Traits), DTOs, Laravel Optimizer
│   ├── corral-platform/           # Windows x64 Infrastructure (Job Object, SQLite WAL, TLS rcgen, SMTP, Dumps)
│   ├── corral-app/                # Binary utama Tauri 2 (68+ IPC commands, System Tray, Window Lifecycle)
│   ├── corral-helper/             # Binary Windows Service (Named Pipe + HMAC/TLD Security Policy)
│   ├── corral-cli/                # Binary CLI `corral` (link, unlink, park, php, node, tls, service, doctor)
│   └── corral-mcp/                # Binary MCP Server (JSON-RPC stdio untuk AI Agent / Cursor / Claude)
└── ui/                            # Frontend Svelte 5 + TypeScript + Vite (Production bundle < 100 KB)
```

---

## Cara Menjalankan & Build (Windows x64)

### 1. Build Frontend UI
```powershell
cd ui
npm install
npm run build
```

### 2. Jalankan Unit Test & Linter (Zero-Crash Policy)
```powershell
cargo clippy --workspace -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::indexing_slicing
cargo test --workspace
```

### 3. Jalankan Aplikasi Desktop Corral
```powershell
cargo run -p corral-app
```

### 4. Build Binary Release Teroptimasi
```powershell
cargo build --workspace --release
```
