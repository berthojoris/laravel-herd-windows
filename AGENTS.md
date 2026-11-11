# Panduan Agen untuk Corral

## Konteks proyek

- Corral adalah aplikasi desktop pengembangan PHP/Laravel untuk Windows x64, menggunakan Rust, Tauri 2, Svelte 5, TypeScript, dan Vite. Target utama adalah `x86_64-pc-windows-msvc`.
- Baca `PRD_CORAL.md` dan kode terkait sebelum mengubah perilaku; periksa implementasi aktual karena dokumentasi dan beberapa fitur yang masih berupa simulasi belum tentu mencerminkan kondisi runtime.
- Pertahankan perubahan pengguna yang sudah ada. Jangan menghapus data situs, database, sertifikat, konfigurasi, atau file pengguna tanpa alasan dan otorisasi yang sesuai.

## Struktur dan batas tanggung jawab

- `crates/corral-core`: domain, DTO, aturan aplikasi, dan trait port. Jaga logika domain tetap terpisah dari integrasi Windows dan UI.
- `crates/corral-platform`: implementasi sistem Windows, proses, file, SQLite, Nginx, TLS, SMTP, dan dump.
- `crates/corral-app`: aplikasi Tauri, command IPC, state, dan tray. Daftarkan command baru pada `src/lib.rs` dan selaraskan kontrak frontend di `ui/src/lib/ipc.ts`.
- `crates/corral-helper`, `crates/corral-cli`, `crates/corral-mcp`: helper, CLI, dan server MCP; pertahankan kompatibilitas kontrak yang digunakan bersama.
- `ui/src`: Svelte dan stylesheet sumber. `ui/dist`, `target`, dan `crates/corral-app/gen` merupakan keluaran build atau file yang dihasilkan; ubah sumbernya, bukan hasil generate secara manual.

## Rust dan Windows

- Ikuti edition dan `rust-version` pada workspace; gunakan dependensi workspace yang tersedia dan perbarui lockfile jika dependensi berubah.
- Tangani kegagalan dengan `Result` dan `AppError` yang memberikan konteks. Patuhi lint workspace untuk `unwrap`, `expect`, `panic`, dan indexing. Jangan menyembunyikan kegagalan yang memengaruhi hasil operasi atau melaporkan keberhasilan palsu.
- Hindari operasi I/O, proses yang lama, dan network yang memblokir thread UI atau runtime async. Jangan mempertahankan `MutexGuard` selama `.await` atau proses eksternal yang lama; ambil data yang diperlukan, lepas lock, lalu jalankan pekerjaan.
- Gunakan `Path`/`PathBuf`, validasi input dan versi, serta tangani path Windows yang mengandung spasi. Jalankan executable dengan argumen terpisah; hindari membentuk perintah shell dari input pengguna.
- Kelola proses anak dan handle secara eksplisit, termasuk penghentian dan pelaporan status sebenarnya. Gunakan mekanisme supervisi dan Job Object proyek saat menambahkan layanan yang dikelola.
- Jaga kompatibilitas data SQLite dan pengaturan yang telah tersimpan. Field pengaturan baru harus memiliki default atau migrasi yang aman; jangan menimpa data lama secara diam-diam.

## Tauri, keamanan, dan integrasi

- Anggap data dari WebView dan IPC tidak tepercaya. Validasi nama, path, URL, dan operasi di backend; batasi akses ke resource yang relevan. Periksa hasil operasi filesystem, proses, dan reload sebelum melaporkan sukses.
- Pertahankan CSP dan capability sekecil mungkin pada `crates/corral-app/tauri.conf.json` dan `crates/corral-app/capabilities/`. Jangan memperluas izin atau mengekspos command sensitif tanpa kebutuhan yang jelas.
- Perlakukan hosts Windows, sertifikat CA, helper, dan proses dengan hak istimewa sebagai batas kepercayaan. Jangan mengaku helper aktif, CA dipercaya, service berjalan, atau perubahan sistem berhasil tanpa verifikasi nyata.
- Jangan mengganti implementasi nyata dengan data contoh atau simulasi tersembunyi. Jika fitur belum tersedia, tampilkan status dan kegagalan yang jelas.
- Untuk pengaturan Nginx, PHP, dan Laravel, pertahankan file pengguna dan gunakan penulisan yang aman. Validasi konfigurasi sebelum reload dan laporkan kegagalan reload.

## Frontend dan pengalaman pengguna

- Gunakan pola Svelte 5, tipe TypeScript, komponen dan token CSS yang sudah ada. Pertahankan aksesibilitas keyboard, label, fokus, dan tampilan pada ukuran jendela yang didukung.
- Tampilkan notifikasi proses dalam modal di tengah layar. Tombol yang menjalankan operasi async harus dinonaktifkan dan menampilkan spinner hingga proses selesai atau gagal; gunakan pola bersama yang sudah ada.
- Jangan menampilkan klaim status, versi, metrik, atau hasil proses sebagai fakta jika belum diverifikasi oleh backend. Jelaskan error dan tindakan yang dapat dilakukan pengguna.

## Build release EXE wajib

- **Setelah setiap perubahan kode** Rust, Svelte, TypeScript, CSS, konfigurasi aplikasi, atau dependensi yang memengaruhi aplikasi, selalu build ulang EXE release sebelum menyatakan pekerjaan selesai, kecuali instruksi pengguna yang lebih spesifik melarang atau lingkungan menghalangi build. Perubahan dokumentasi saja tidak memerlukan build.
- Di root workspace pada Windows, build UI terlebih dahulu agar aset terbaru tertanam, kemudian build aplikasi:

  ```powershell
  npm --prefix ui run build
  cargo build --release -p corral-app
  ```

- Tunggu kedua perintah selesai dan pastikan `target/release/corral.exe` benar-benar dihasilkan. Laporkan lokasi EXE, hasil build, dan kendala bila build gagal atau tidak dapat dijalankan. Jangan menyajikan EXE lama sebagai hasil baru.
- Build ini menghasilkan executable, bukan installer. Pembuatan installer NSIS memerlukan alur bundling Tauri terpisah jika diminta.

## Pemeriksaan dan referensi

- Gunakan pemeriksaan yang relevan dengan perubahan dan instruksi pengguna. CI proyek mendokumentasikan `cargo clippy --workspace -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::indexing_slicing` dan `cargo test --workspace`; laporkan pemeriksaan yang dijalankan dan kegagalannya secara akurat.
- Rujukan resmi: [Tauri capabilities](https://v2.tauri.app/security/capabilities/), [Tauri security](https://v2.tauri.app/security/), dan [Rust Clippy](https://doc.rust-lang.org/clippy/usage.html).
