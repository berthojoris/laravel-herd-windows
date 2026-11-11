<script lang="ts">
  import { onMount } from 'svelte';
  import {
    call,
    type AppInfoView,
    type AppSettingsDto,
    type ArtisanOutputDto,
    type CertStatusView,
    type DatabaseView,
    type DbCredentialsView,
    type DoctorReportView,
    type DumpEntry,
    type HelperStatusView,
    type LaravelHealthReport,
    type LogFileView,
    type LogLine,
    type MailMessage,
    type MailSettingsView,
    type MailSummaryView,
    type NodeVersionView,
    type PhpIniSettingsDto,
    type PhpVersionView,
    type ServiceView,
    type SiteView,
    type WorkerKind,
    type WorkspaceView,
  } from './lib/ipc';
  import { messageFor, parseAppError } from './lib/errors';
  import appIcon from './icon.png';

  type NavTab = 'applications' | 'services' | 'mail' | 'dumps' | 'logs' | 'settings';
  type SiteSubTab = 'general' | 'laravel' | 'workers' | 'env' | 'workspaces';
  type ServiceSubTab = 'daemons' | 'phpini' | 'databases';

  let activeTab = $state<NavTab>('applications');
  let siteSubTab = $state<SiteSubTab>('general');
  let serviceSubTab = $state<ServiceSubTab>('daemons');

  let loading = $state<boolean>(true);
  let notifications = $state<{ message: string; kind: 'info' | 'error' | 'success' }[]>([]);

  let appInfo = $state<AppInfoView | null>(null);
  let settings = $state<AppSettingsDto | null>(null);
  let sites = $state<SiteView[]>([]);
  let selectedSiteId = $state<string>('');
  let searchQuery = $state<string>('');

  let services = $state<ServiceView[]>([]);
  let phpVersions = $state<PhpVersionView[]>([]);
  let nodeVersions = $state<NodeVersionView[]>([]);
  let phpIni = $state<PhpIniSettingsDto | null>(null);
  let selectedPhpForIni = $state<string>('8.4');

  let databases = $state<DatabaseView[]>([]);
  let dbCredentials = $state<DbCredentialsView | null>(null);
  let newDbName = $state<string>('');

  let mails = $state<MailSummaryView[]>([]);
  let mailSettings = $state<MailSettingsView | null>(null);
  let selectedMailId = $state<string>('');
  let selectedMailDetail = $state<MailMessage | null>(null);
  let loadingMailDetail = $state<boolean>(false);
  let mailPreviewMode = $state<'html' | 'text' | 'headers'>('html');

  let dumps = $state<DumpEntry[]>([]);
  let logFiles = $state<LogFileView[]>([]);
  let selectedLogPath = $state<string>('');
  let logLines = $state<LogLine[]>([]);
  let logFilterLevel = $state<string>('ALL');
  let logSearch = $state<string>('');

  let workspaces = $state<WorkspaceView[]>([]);
  let newWorkspaceBranch = $state<string>('feature-payment-gateway');

  let certStatus = $state<CertStatusView | null>(null);
  let helperStatus = $state<HelperStatusView | null>(null);
  let doctorReport = $state<DoctorReportView | null>(null);

  let laravelHealth = $state<LaravelHealthReport | null>(null);
  let artisanOutput = $state<ArtisanOutputDto | null>(null);
  let customArtisanCmd = $state<string>('about');
  let siteEnvContent = $state<string>('');

  // Modal states
  let showNewSiteModal = $state<boolean>(false);
  let newSiteName = $state<string>('');
  let newSitePhp = $state<string>('8.4');
  let newSiteHttps = $state<boolean>(true);
  let newSiteTemplate = $state<string>('laravel');
  let creatingSite = $state<boolean>(false);

  let showLinkFolderModal = $state<boolean>(false);
  let linkFolderPath = $state<string>('');
  let linkSiteName = $state<string>('');

  let showExtraServiceModal = $state<boolean>(false);
  let extraSvcName = $state<string>('MySQL 8.0 Secondary');
  let extraSvcKind = $state<string>('mariaDb');
  let extraSvcVersion = $state<string>('8.0.40');
  let extraSvcPort = $state<number>(3307);

  // Context menu & site delete confirmation modal
  let siteContextMenu = $state<{ x: number; y: number; site: SiteView } | null>(null);
  let siteToDelete = $state<SiteView | null>(null);

  function handleSiteContextMenu(e: MouseEvent, site: SiteView) {
    e.preventDefault();
    e.stopPropagation();
    selectedSiteId = site.id;
    const menuWidth = 200;
    const menuHeight = 160;
    const x = Math.min(e.clientX, window.innerWidth - menuWidth - 8);
    const y = Math.min(e.clientY, window.innerHeight - menuHeight - 8);
    siteContextMenu = { x, y, site };
  }

  function closeSiteContextMenu() {
    siteContextMenu = null;
  }

  function promptDeleteSite(site: SiteView) {
    siteContextMenu = null;
    siteToDelete = site;
  }

  const filteredSites = $derived(
    sites.filter(
      (s) =>
        s.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        s.host.toLowerCase().includes(searchQuery.toLowerCase()) ||
        s.path.toLowerCase().includes(searchQuery.toLowerCase()),
    ),
  );

  const selectedSite = $derived(
    sites.find((s) => s.id === selectedSiteId) ?? sites[0] ?? null,
  );

  const selectedMail = $derived(
    mails.find((m) => m.id === selectedMailId) ?? mails[0] ?? null,
  );

  const unreadMailsCount = $derived(mails.filter((m) => !m.read).length);

  const filteredLogLines = $derived(
    logLines.filter((l) => {
      const levelOk =
        logFilterLevel === 'ALL' || l.level.toUpperCase().includes(logFilterLevel);
      const searchOk =
        !logSearch || l.text.toLowerCase().includes(logSearch.toLowerCase());
      return levelOk && searchOk;
    }),
  );

  const MAX_NOTIFICATIONS = 5;

  function notify(msg: string, kind: 'info' | 'error' | 'success' = 'success') {
    const last = notifications[notifications.length - 1];
    if (last && last.message === msg && last.kind === kind) {
      return;
    }
    const next = [...notifications, { message: msg, kind }];
    notifications =
      next.length > MAX_NOTIFICATIONS ? next.slice(next.length - MAX_NOTIFICATIONS) : next;
  }

  function focusNotification(node: HTMLElement) {
    const previous = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => node.querySelector<HTMLButtonElement>('button')?.focus());
    return { destroy: () => { if (previous?.isConnected) previous.focus(); } };
  }

  function dismissNotification() {
    notifications = notifications.slice(1);
  }

  async function runButton(event: MouseEvent, action: () => unknown) {
    const button = event.currentTarget as HTMLButtonElement;
    if (button.dataset.pending) return;
    button.dataset.pending = 'true';
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    try {
      await action();
    } catch (error) {
      notify(messageFor(parseAppError(error)), 'error');
    } finally {
      delete button.dataset.pending;
      button.removeAttribute('aria-busy');
      if (button.isConnected) button.disabled = false;
    }
  }

  function applyThemeToDocument(themePref: string) {
    if (themePref === 'light') {
      document.documentElement.setAttribute('data-theme', 'light');
    } else if (themePref === 'dark') {
      document.documentElement.setAttribute('data-theme', 'dark');
    } else {
      const prefersLight = window.matchMedia('(prefers-color-scheme: light)').matches;
      document.documentElement.setAttribute('data-theme', prefersLight ? 'light' : 'dark');
    }
  }

  let refreshPromise: Promise<void> | null = null;

  async function selectMail(id: string) {
    selectedMailId = id;
    if (!id) {
      selectedMailDetail = null;
      return;
    }
    loadingMailDetail = true;
    try {
      const detail = await call<MailMessage>('get_mail', { id });
      if (selectedMailId === id) {
        selectedMailDetail = detail;
      }
      const idx = mails.findIndex((m) => m.id === id);
      if (idx !== -1 && !mails[idx].read) {
        mails[idx] = { ...mails[idx], read: true };
        if (mailSettings && mailSettings.unreadCount > 0) {
          mailSettings = { ...mailSettings, unreadCount: mailSettings.unreadCount - 1 };
        }
      }
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    } finally {
      if (selectedMailId === id) {
        loadingMailDetail = false;
      }
    }
  }

  async function refreshAll(): Promise<void> {
    if (refreshPromise) {
      return refreshPromise;
    }
    refreshPromise = performRefreshAll().finally(() => {
      refreshPromise = null;
    });
    return refreshPromise;
  }

  async function performRefreshAll() {
    try {
      const [
        infoRes,
        settingsRes,
        sitesRes,
        servicesRes,
        phpRes,
        nodeRes,
        mailsRes,
        mailSetRes,
        dumpsRes,
        logsRes,
        wsRes,
        dbsRes,
        dbCredRes,
        certRes,
        helperRes,
      ] = await Promise.all([
        call<AppInfoView>('app_info'),
        call<AppSettingsDto>('get_settings'),
        call<SiteView[]>('list_sites'),
        call<ServiceView[]>('list_services'),
        call<PhpVersionView[]>('list_php_versions'),
        call<NodeVersionView[]>('list_node_versions'),
        call<MailSummaryView[]>('list_mails'),
        call<MailSettingsView>('mail_settings'),
        call<DumpEntry[]>('list_dumps'),
        call<LogFileView[]>('list_log_files'),
        call<WorkspaceView[]>('list_workspaces'),
        call<DatabaseView[]>('list_databases'),
        call<DbCredentialsView>('db_credentials'),
        call<CertStatusView>('cert_status'),
        call<HelperStatusView>('helper_status'),
      ]);

      appInfo = infoRes;
      settings = settingsRes;
      applyThemeToDocument(settingsRes.theme);
      sites = sitesRes;
      if (!selectedSiteId && sitesRes.length > 0) {
        selectedSiteId = sitesRes[0]?.id ?? '';
      }
      services = servicesRes;
      phpVersions = phpRes;
      nodeVersions = nodeRes;
      mails = mailsRes;
      if (mailsRes.length === 0) {
        selectedMailId = '';
        selectedMailDetail = null;
      } else if (!selectedMailId || !mailsRes.some((m) => m.id === selectedMailId)) {
        selectedMailId = mailsRes[0]?.id ?? '';
        selectedMailDetail = null;
      }
      mailSettings = mailSetRes;
      dumps = dumpsRes;
      logFiles = logsRes;
      if (!selectedLogPath && logsRes.length > 0) {
        selectedLogPath = logsRes[0]?.path ?? '';
      }
      workspaces = wsRes;
      databases = dbsRes;
      dbCredentials = dbCredRes;
      certStatus = certRes;
      helperStatus = helperRes;

      if (selectedSiteId) {
        await loadSiteExtras(selectedSiteId);
      }
      if (activeTab === 'logs' && selectedLogPath) {
        await loadLogContent(selectedLogPath);
      }
      if (
        activeTab === 'mail' &&
        selectedMailId &&
        (!selectedMailDetail || selectedMailDetail.id !== selectedMailId)
      ) {
        await selectMail(selectedMailId);
      }
    } catch (e) {
      const err = parseAppError(e);
      notify(messageFor(err), 'error');
    } finally {
      loading = false;
    }
  }

  async function loadSiteExtras(siteId: string) {
    try {
      const [health, envStr] = await Promise.all([
        call<LaravelHealthReport>('laravel_health_check', { siteId }),
        call<string>('get_site_env', { siteId }),
      ]);
      laravelHealth = health;
      siteEnvContent = envStr;
    } catch {
      // Non-fatal if site was deleted
    }
  }

  async function selectSite(site: SiteView) {
    selectedSiteId = site.id;
    artisanOutput = null;
    await loadSiteExtras(site.id);
  }

  async function handleCreateSite() {
    if (creatingSite) return;
    if (!newSiteName.trim()) {
      notify('Nama proyek Laravel wajib diisi', 'error');
      return;
    }
    creatingSite = true;
    try {
      const created = await call<SiteView>('create_site', {
        name: newSiteName.trim(),
        phpVersion: newSitePhp,
        https: newSiteHttps,
        template: newSiteTemplate,
      });
      showNewSiteModal = false;
      newSiteName = '';
      await refreshAll();
      selectedSiteId = created.id;
      await loadSiteExtras(created.id);
      notify(`Situs Laravel ${created.host} berhasil dibuat!`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    } finally {
      creatingSite = false;
    }
  }

  async function handleLinkFolder() {
    if (!linkFolderPath.trim()) {
      notify('Path folder proyek wajib diisi', 'error');
      return;
    }
    try {
      const linked = await call<SiteView>('link_folder', {
        path: linkFolderPath.trim(),
        name: linkSiteName.trim() || null,
      });
      showLinkFolderModal = false;
      linkFolderPath = '';
      linkSiteName = '';
      await refreshAll();
      selectedSiteId = linked.id;
      notify(`Folder berhasil di-link ke ${linked.host}`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleDeleteSite(siteId: string, host: string) {
    try {
      await call<void>('delete_site', { id: siteId });
      selectedSiteId = '';
      await refreshAll();
      notify(`Situs ${host} dihapus dari daftar Corral`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleToggleSiteHttps(site: SiteView) {
    try {
      const cmd = site.https ? 'disable_https' : 'enable_https';
      await call<SiteView>(cmd, { siteId: site.id });
      await refreshAll();
      notify(`HTTPS untuk ${site.host} berhasil ${site.https ? 'dinonaktifkan' : 'diaktifkan (TLS)'}`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSwitchPhpSite(siteId: string, phpVersion: string) {
    try {
      await call<SiteView>('switch_php_site', { siteId, phpVersion });
      await refreshAll();
      notify(`Versi PHP situs berhasil diubah ke PHP ${phpVersion} (< 300ms reload)`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSwitchPhpGlobal(phpVersion: string) {
    try {
      await call<void>('switch_php_global', { version: phpVersion });
      await refreshAll();
      notify(`Versi PHP Global diubah ke PHP ${phpVersion}`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSwitchNodeSite(siteId: string, nodeVersion: string) {
    try {
      await call<SiteView>('switch_node_site', { siteId, nodeVersion });
      await refreshAll();
      notify(`Node LTS situs diubah ke v${nodeVersion}`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleOptimizeLaravel(siteId: string) {
    try {
      const report = await call<LaravelHealthReport>('optimize_laravel_site', { siteId });
      laravelHealth = report;
      await refreshAll();
      notify('Optimasi Server Laravel (OPcache JIT 128M + Realpath 4096K + .env) berhasil diterapkan!');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleRunArtisan(siteId: string, command: string) {
    try {
      const out = await call<ArtisanOutputDto>('run_artisan_command', { siteId, command });
      artisanOutput = out;
      await loadSiteExtras(siteId);
      notify(`php artisan ${command} selesai dalam ${out.durationMs} ms`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleToggleWorker(siteId: string, worker: WorkerKind) {
    try {
      await call<SiteView>('toggle_site_worker', { siteId, worker });
      await refreshAll();
      notify(`Worker Laravel '${worker}' diperbarui`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSaveSiteEnv(siteId: string) {
    try {
      await call<void>('save_site_env', { siteId, content: siteEnvContent });
      await loadSiteExtras(siteId);
      notify('File .env berhasil disimpan secara atomik');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleAutofillSiteEnv(siteId: string) {
    try {
      const updated = await call<string>('autofill_site_env', { siteId });
      siteEnvContent = updated;
      await loadSiteExtras(siteId);
      notify('.env berhasil disinkronkan dengan DB, Redis, Mail Catcher (:1025) & Dumps (:9912)!');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleCreateWorkspace(siteId: string) {
    if (!newWorkspaceBranch.trim()) return;
    try {
      await call<WorkspaceView>('create_workspace', {
        siteId,
        branch: newWorkspaceBranch.trim(),
      });
      await refreshAll();
      notify(`Workspace git worktree '${newWorkspaceBranch}' berhasil dibuat!`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleDeleteWorkspace(id: string) {
    try {
      await call<void>('delete_workspace', { id });
      await refreshAll();
      notify('Workspace dihapus dan database cabangnya telah dibersihkan');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleServiceAction(id: string, action: 'start' | 'stop' | 'restart') {
    try {
      const cmd =
        action === 'start'
          ? 'start_service'
          : action === 'stop'
            ? 'stop_service'
            : 'restart_service';
      await call<ServiceView>(cmd, { id });
      await refreshAll();
      notify(`Service ${id} berhasil di-${action}`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleStartOrStopAll(start: boolean) {
    try {
      await call<void>(start ? 'start_all' : 'stop_all');
      await refreshAll();
      notify(start ? 'Seluruh service Corral dijalankan' : 'Seluruh service Corral dihentikan');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function loadPhpIni(ver: string) {
    selectedPhpForIni = ver;
    try {
      phpIni = await call<PhpIniSettingsDto>('get_php_ini', { version: ver });
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSavePhpIni() {
    if (!phpIni) return;
    try {
      await call<void>('set_php_ini', { settings: phpIni });
      await refreshAll();
      notify(`php.ini untuk PHP ${phpIni.version} berhasil disimpan dan di-reload!`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleCreateDatabase() {
    if (!newDbName.trim()) return;
    try {
      await call<DatabaseView>('create_database', { name: newDbName.trim() });
      newDbName = '';
      await refreshAll();
      notify('Database baru berhasil dibuat di MariaDB 127.0.0.1:3306');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleDropDatabase(name: string) {
    try {
      await call<void>('drop_database', { name });
      await refreshAll();
      notify(`Database '${name}' dihapus`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSendTestMail() {
    try {
      const m = await call<MailMessage>('send_test_mail');
      selectedMailId = m.id;
      selectedMailDetail = m;
      await refreshAll();
      notify('Email uji Laravel berhasil ditangkap oleh Corral SMTP (:1025)!');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleClearMails() {
    try {
      await call<void>('clear_mails');
      selectedMailId = '';
      selectedMailDetail = null;
      await refreshAll();
      notify('Semua email di Mail Catcher dibersihkan');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSendTestDump() {
    try {
      await call<DumpEntry>('send_test_dump');
      await refreshAll();
      notify('Payload dump($model) baru diterima di panel Dumps!');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function loadLogContent(path: string) {
    selectedLogPath = path;
    try {
      logLines = await call<LogLine[]>('read_log_lines', { path });
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleClearLog() {
    if (!selectedLogPath) return;
    try {
      await call<void>('clear_log', { path: selectedLogPath });
      await loadLogContent(selectedLogPath);
      notify('File log berhasil dikosongkan');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleRunDoctor() {
    try {
      doctorReport = await call<DoctorReportView>('doctor');
      notify('Corral Doctor selesai melakukan diagnostik sistem Windows x64');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleSaveSettings() {
    if (!settings) return;
    try {
      await call<void>('save_settings_cmd', { settings });
      applyThemeToDocument(settings.theme);
      await refreshAll();
      notify('Pengaturan Corral & System Tray berhasil disimpan');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleMinimizeToTrayNow() {
    try {
      await call<void>('minimize_to_tray');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleTrustCa(trust: boolean) {
    try {
      await call<CertStatusView>(trust ? 'trust_ca' : 'untrust_ca');
      await refreshAll();
      notify(trust ? 'Corral Local CA dipasang ke Windows Trust Store' : 'Corral Local CA dicabut');
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  async function handleCreateExtraService() {
    try {
      await call<ServiceView>('create_extra_service', {
        name: extraSvcName,
        kind: extraSvcKind,
        version: extraSvcVersion,
        port: Number(extraSvcPort),
      });
      showExtraServiceModal = false;
      await refreshAll();
      notify(`Service tambahan ${extraSvcName} berhasil ditambahkan`);
    } catch (e) {
      notify(messageFor(parseAppError(e)), 'error');
    }
  }

  onMount(() => {
    void refreshAll();
  });
</script>

<div class="app-shell">
  <!-- TOPBAR HERD-STYLE -->
  <header class="topbar">
    <div class="brand">
      <img class="brand-logo-img" src={appIcon} alt="Corral" />
      <span>Corral</span>
      <span class="brand-subtitle">Laravel &amp; PHP Server · Windows x64</span>
    </div>

    <div class="topbar-controls">
      {#if settings}
        <label style="display:flex;align-items:center;gap:6px;font-size:12px;color:var(--text-secondary);">
          <span>Global PHP:</span>
          <select
            class="select"
            value={settings.defaultPhpVersion}
            onchange={(e) => handleSwitchPhpGlobal((e.currentTarget as HTMLSelectElement).value)}
          >
            {#each phpVersions as pv}
              <option value={pv.version}>PHP {pv.version} (: {pv.fastcgiPort})</option>
            {/each}
          </select>
        </label>
      {/if}

      <button class="btn btn-sm" onclick={(event) => runButton(event, () => (() => handleStartOrStopAll(true))())} title="Start All Services">
        ● Start All
      </button>
      <button class="btn btn-sm" onclick={(event) => runButton(event, () => (() => refreshAll())())} title="Reload Status">
        ⟳ Reload
      </button>
      <button
        class="btn btn-sm"
        onclick={(event) => runButton(event, () => (handleMinimizeToTrayNow)())}
        title="Sembunyikan jendela ke System Tray Windows (Server tetap berjalan di background)"
      >
        ▾ Minimize to Tray
      </button>
    </div>
  </header>

  <div class="main-body">
    <!-- LEFT SIDEBAR (220px) -->
    <aside class="sidebar">
      <nav class="nav-list" aria-label="Menu Utama Corral">
        <button
          class="nav-btn {activeTab === 'applications' ? 'active' : ''}"
          onclick={() => (activeTab = 'applications')}
        >
          <span class="nav-btn-left">
            <span>◈</span>
            <span>Applications</span>
          </span>
          <span class="badge-count">{sites.length}</span>
        </button>

        <button
          class="nav-btn {activeTab === 'services' ? 'active' : ''}"
          onclick={(event) => runButton(event, () => (() => {
            activeTab = 'services';
            if (!phpIni) return loadPhpIni(selectedPhpForIni);
          })())}
        >
          <span class="nav-btn-left">
            <span>⚡</span>
            <span>Services &amp; PHP</span>
          </span>
          <span class="badge-count">
            {services.filter((s) => s.state === 'running').length}/{services.length}
          </span>
        </button>

        <button
          class="nav-btn {activeTab === 'mail' ? 'active' : ''}"
          onclick={() => {
            activeTab = 'mail';
            const targetId = selectedMailId || mails[0]?.id || '';
            if (targetId && (!selectedMailDetail || selectedMailDetail.id !== targetId)) {
              void selectMail(targetId);
            }
          }}
        >
          <span class="nav-btn-left">
            <span>✉</span>
            <span>Mail Catcher</span>
          </span>
          {#if unreadMailsCount > 0}
            <span class="badge-count" style="background:var(--accent);color:#fff;border:none;">
              {unreadMailsCount}
            </span>
          {:else}
            <span class="badge-count">{mails.length}</span>
          {/if}
        </button>

        <button
          class="nav-btn {activeTab === 'dumps' ? 'active' : ''}"
          onclick={() => (activeTab = 'dumps')}
        >
          <span class="nav-btn-left">
            <span>🐞</span>
            <span>Dumps</span>
          </span>
          <span class="badge-count">{dumps.length}</span>
        </button>

        <button
          class="nav-btn {activeTab === 'logs' ? 'active' : ''}"
          onclick={(event) => runButton(event, () => (() => {
            activeTab = 'logs';
            if (selectedLogPath) return loadLogContent(selectedLogPath);
          })())}
        >
          <span class="nav-btn-left">
            <span>☰</span>
            <span>Logs</span>
          </span>
        </button>

        <button
          class="nav-btn {activeTab === 'settings' ? 'active' : ''}"
          onclick={(event) => runButton(event, () => (() => {
            activeTab = 'settings';
            if (!doctorReport) return handleRunDoctor();
          })())}
        >
          <span class="nav-btn-left">
            <span>⚙</span>
            <span>Settings &amp; Doctor</span>
          </span>
        </button>
      </nav>

      <!-- LIVE SERVICE STATUS FOOTER -->
      <div class="sidebar-status-box">
        <div class="sidebar-section-title">
          <span>Daemon Status</span>
          <button
            class="btn btn-sm"
            style="padding:1px 6px;font-size:10px;"
            onclick={(event) => runButton(event, () => (() => handleStartOrStopAll(false))())}
          >
            Stop All
          </button>
        </div>

        {#each services.slice(0, 6) as svc}
          <div class="mini-svc-row">
            <span style="display:flex;align-items:center;gap:7px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
              <span
                class="dot {svc.state === 'running'
                  ? 'up'
                  : svc.state === 'starting'
                    ? 'starting'
                    : svc.state === 'failed'
                      ? 'failed'
                      : 'off'}"
              ></span>
              <span>{svc.name.replace(' Server', '').replace(' Pool', '')}</span>
            </span>
            <span style="color:var(--text-muted);font-size:11px;">:{svc.port}</span>
          </div>
        {/each}
      </div>
    </aside>

    <!-- MAIN CONTENT WORKSPACE -->
    <main class="workspace-pane">
      {#if helperStatus && !helperStatus.installed}
        <div class="degraded-banner">
          <span>
            <strong>Mode Non-Admin Aktif:</strong> Service <code>CorralHelper</code> belum dipasang. Server PHP/Laravel tetap berjalan normal; pasang helper untuk otomatisasi file <code>hosts</code> tanpa UAC.
          </span>
          <button
            class="btn btn-sm btn-primary"
            onclick={(event) => runButton(event, () => (async () => {
              await call('install_helper');
              await refreshAll();
              notify('CorralHelper berhasil diaktifkan!');
            })())}
          >
            Pasang Helper
          </button>
        </div>
      {/if}

      {#if loading}
        <div style="padding:32px;color:var(--text-secondary);">
          Memuat konfigurasi aplikasi Laravel &amp; service Corral...
        </div>
      {:else if activeTab === 'applications'}
        <!-- APPLICATIONS VIEW (HERD MASTER-DETAIL) -->
        <div class="split-view">
          <div class="list-column">
            <div class="toolbar">
              <input
                class="input"
                type="search"
                placeholder="Cari situs .test atau folder..."
                bind:value={searchQuery}
              />
              <button class="btn btn-primary btn-sm" onclick={() => (showNewSiteModal = true)}>
                + New
              </button>
              <button class="btn btn-sm" onclick={() => (showLinkFolderModal = true)}>
                Link
              </button>
            </div>

            <div class="scroll-list">
              {#if filteredSites.length === 0}
                <div style="padding:28px 18px;text-align:center;color:var(--text-secondary);">
                  <p style="margin-bottom:10px;font-weight:600;">Belum ada situs yang cocok.</p>
                  <button class="btn btn-primary btn-sm" onclick={() => (showNewSiteModal = true)}>
                    + Buat Proyek Laravel Baru
                  </button>
                </div>
              {:else}
                {#each filteredSites as site (site.id)}
                  <div
                    class="site-item {selectedSite?.id === site.id ? 'selected' : ''}"
                    role="button"
                    tabindex="0"
                    onclick={() => selectSite(site)}
                    onkeydown={(e) => e.key === 'Enter' && selectSite(site)}
                    oncontextmenu={(e) => handleSiteContextMenu(e, site)}
                  >
                    <div class="site-item-top">
                      <span class="site-domain">
                        <span class="dot {site.healthy ? 'up' : 'off'}"></span>
                        <span>{site.host}</span>
                      </span>
                      <div style="display:flex;gap:4px;">
                        {#if site.https}
                          <span class="pill pill-https">HTTPS</span>
                        {:else}
                          <span class="pill">HTTP</span>
                        {/if}
                        <span class="pill pill-php">PHP {site.phpVersion}</span>
                      </div>
                    </div>

                    <div class="site-meta">
                      <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:210px;">
                        {site.path}
                      </span>
                      <span class="pill {site.isLaravel ? 'pill-laravel' : ''}">
                        {site.frameworkBadge}
                      </span>
                    </div>
                  </div>
                {/each}
              {/if}
            </div>
          </div>

          <!-- RIGHT DETAIL COLUMN -->
          <div class="detail-column">
            {#if selectedSite}
              <div class="site-header">
                <!-- ROW 1: Domain & Action Buttons -->
                <div class="site-header-top">
                  <div class="site-title-group">
                    <span class="dot {selectedSite.healthy ? 'up' : 'off'}"></span>
                    <h2 class="site-title">{selectedSite.host}</h2>
                  </div>

                  <div class="site-actions">
                    <button
                      class="btn btn-primary btn-sm"
                      onclick={(event) => runButton(event, () => (() => call('open_in_browser', { siteId: selectedSite.id }))())}
                      title="Buka situs di browser"
                    >
                      <span>↗</span> Open
                    </button>
                    <button
                      class="btn btn-sm"
                      onclick={(event) => runButton(event, () => (() => call('open_in_editor', { siteId: selectedSite.id }))())}
                      title="Buka proyek di Code Editor"
                    >
                      <span>&lt;/&gt;</span> Editor
                    </button>
                    <button
                      class="btn btn-sm"
                      onclick={(event) => runButton(event, () => (() => call('open_path', { path: selectedSite.path }))())}
                      title="Buka folder di File Explorer"
                    >
                      <span>📁</span> Folder
                    </button>
                    <button
                      class="btn btn-danger btn-sm"
                      onclick={() => promptDeleteSite(selectedSite)}
                      title="Hapus situs dari daftar Corral"
                    >
                      Unlink
                    </button>
                  </div>
                </div>

                <!-- ROW 2: URL, Document Root & Status Badges -->
                <div class="site-header-meta">
                  <div class="site-path-info">
                    <a
                      href={selectedSite.url}
                      target="_blank"
                      rel="noreferrer"
                      class="site-url-link"
                    >
                      {selectedSite.url}
                    </a>
                    <span>·</span>
                    <span class="site-docroot" title={selectedSite.documentRoot}>
                      <code>{selectedSite.documentRoot}</code>
                    </span>
                  </div>

                  <div class="site-badges-group">
                    <span class="pill pill-laravel">{selectedSite.frameworkBadge}</span>
                    <span class="pill pill-php">PHP {selectedSite.phpVersion}</span>
                    {#if selectedSite.opcacheEnabled}
                      <span class="pill pill-https">⚡ OPcache Turbo</span>
                    {/if}
                    {#if selectedSite.hasCorralYml}
                      <span class="pill">.corral.yml ✓</span>
                    {/if}
                  </div>
                </div>
              </div>

              <!-- SITE SUBTABS -->
              <div class="subtabs">
                <button
                  class="subtab-btn {siteSubTab === 'general' ? 'active' : ''}"
                  onclick={() => (siteSubTab = 'general')}
                >
                  General &amp; Routing
                </button>
                <button
                  class="subtab-btn {siteSubTab === 'laravel' ? 'active' : ''}"
                  onclick={() => (siteSubTab = 'laravel')}
                >
                  🚀 Laravel App Server Optimizer
                  {#if laravelHealth}
                    ({laravelHealth.score}%)
                  {/if}
                </button>
                <button
                  class="subtab-btn {siteSubTab === 'workers' ? 'active' : ''}"
                  onclick={() => (siteSubTab = 'workers')}
                >
                  Workers &amp; Reverb ({selectedSite.workers.length})
                </button>
                <button
                  class="subtab-btn {siteSubTab === 'env' ? 'active' : ''}"
                  onclick={() => (siteSubTab = 'env')}
                >
                  Environment (.env)
                </button>
                <button
                  class="subtab-btn {siteSubTab === 'workspaces' ? 'active' : ''}"
                  onclick={() => (siteSubTab = 'workspaces')}
                >
                  Workspaces
                </button>
              </div>

              {#if siteSubTab === 'general'}
                <div class="card">
                  <div class="card-header">
                    <span class="card-title">Konfigurasi Server Situs (Zero-Downtime Reload)</span>
                    <button
                      class="btn btn-sm"
                      onclick={(event) => runButton(event, () => (() => call('open_path', { path: selectedSite.path }))())}
                    >
                      Buka di Explorer
                    </button>
                  </div>

                  <div class="kv-grid">
                    <span class="kv-label">Project Path</span>
                    <code>{selectedSite.path}</code>

                    <span class="kv-label">Document Root</span>
                    <code>{selectedSite.documentRoot}</code>

                    <span class="kv-label">PHP Runtime Isolation</span>
                    <div style="display:flex;gap:6px;flex-wrap:wrap;">
                      {#each ['8.1', '8.2', '8.3', '8.4', '8.5'] as ver}
                        <button
                          class="btn btn-sm {selectedSite.phpVersion === ver ? 'btn-primary' : ''}"
                          onclick={(event) => runButton(event, () => (() => handleSwitchPhpSite(selectedSite.id, ver))())}
                        >
                          PHP {ver}
                        </button>
                      {/each}
                    </div>

                    <span class="kv-label">Node.js LTS</span>
                    <div style="display:flex;gap:6px;">
                      {#each ['20', '22', '24'] as nver}
                        <button
                          class="btn btn-sm {selectedSite.nodeVersion === nver ? 'btn-primary' : ''}"
                          onclick={(event) => runButton(event, () => (() => handleSwitchNodeSite(selectedSite.id, nver))())}
                        >
                          Node {nver}
                        </button>
                      {/each}
                    </div>

                    <span class="kv-label">TLS / HTTPS</span>
                    <div style="display:flex;align-items:center;gap:10px;">
                      <button
                        class="btn btn-sm {selectedSite.https ? 'btn-success' : ''}"
                        onclick={(event) => runButton(event, () => (() => handleToggleSiteHttps(selectedSite))())}
                      >
                        {selectedSite.https ? '● HTTPS Aktif (TLS 1.3)' : '○ HTTP Saja (Klik untuk HTTPS)'}
                      </button>
                      {#if certStatus}
                        <span style="font-size:11.5px;color:var(--text-secondary);">
                          CA: {certStatus.caTrusted ? 'Trusted ✓' : 'Belum di-trust'} (Valid ≤ 396 hari)
                        </span>
                      {/if}
                    </div>

                    <span class="kv-label">Database Terhubung</span>
                    <div style="display:flex;align-items:center;gap:10px;">
                      <code>{selectedSite.dbName ?? 'default'}</code>
                      <span style="font-size:12px;color:var(--text-secondary);">
                        MariaDB 11.4 (127.0.0.1:3306, user: <code>root</code>)
                      </span>
                    </div>

                    <span class="kv-label">Domain Hostnames</span>
                    <div style="display:flex;gap:6px;flex-wrap:wrap;">
                      {#each selectedSite.hostnames as h}
                        <span class="pill">{h}</span>
                      {/each}
                    </div>
                  </div>
                </div>
              {:else if siteSubTab === 'laravel'}
                <!-- LARAVEL APP SERVER OPTIMIZER PANEL -->
                <div class="card">
                  <div class="card-header">
                    <div>
                      <span class="card-title">
                        Laravel App Server Performance &amp; Windows x64 Tuning
                      </span>
                      <div style="font-size:11.5px;color:var(--text-secondary);">
                        Optimasi khusus NTFS Realpath Cache, OPcache Bytecode + Tracing JIT, dan FastCGI Pool untuk aplikasi Laravel.
                      </div>
                    </div>
                    <div style="display:flex;align-items:center;gap:10px;">
                      {#if laravelHealth}
                        <span class="pill pill-https" style="font-size:12.5px;padding:4px 10px;">
                          Skor Optimasi: {laravelHealth.score}/100
                        </span>
                      {/if}
                      <button
                        class="btn btn-primary"
                        onclick={(event) => runButton(event, () => (() => handleOptimizeLaravel(selectedSite.id))())}
                      >
                        ⚡ Apply 1-Click Laravel Turbo
                      </button>
                    </div>
                  </div>

                  {#if laravelHealth}
                    <table class="table" style="margin-bottom:14px;">
                      <thead>
                        <tr>
                          <th>Parameter Optimasi</th>
                          <th>Status Saat Ini</th>
                          <th>Rekomendasi Corral</th>
                          <th>Dampak di Windows x64</th>
                        </tr>
                      </thead>
                      <tbody>
                        {#each laravelHealth.items as item}
                          <tr>
                            <td>
                              <div style="font-weight:600;">{item.label}</div>
                              <code style="font-size:11px;color:var(--text-secondary);">{item.key}</code>
                            </td>
                            <td>
                              <span class="pill {item.optimized ? 'pill-https' : ''}">
                                {item.optimized ? '✓ ' : '⚠ '}{item.currentValue}
                              </span>
                            </td>
                            <td><code>{item.recommendedValue}</code></td>
                            <td style="font-size:12px;color:var(--text-secondary);">{item.impact}</td>
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  {/if}
                </div>

                <!-- ARTISAN RUNNER -->
                <div class="card">
                  <div class="card-header">
                    <span class="card-title">Laravel Artisan Runner &amp; Cache Manager</span>
                    <div style="display:flex;gap:6px;">
                      <input
                        class="input"
                        style="width:220px;"
                        placeholder="Perintah artisan..."
                        bind:value={customArtisanCmd}
                      />
                      <button
                        class="btn btn-primary btn-sm"
                        onclick={(event) => runButton(event, () => (() => handleRunArtisan(selectedSite.id, customArtisanCmd))())}
                      >
                        ▶ Run Artisan
                      </button>
                    </div>
                  </div>

                  <div style="display:flex;gap:6px;flex-wrap:wrap;margin-bottom:10px;">
                    {#each ['optimize:clear', 'migrate', 'migrate:fresh --seed', 'storage:link', 'route:list', 'about', 'queue:restart'] as cmd}
                      <button
                        class="btn btn-sm"
                        onclick={(event) => runButton(event, () => (() => handleRunArtisan(selectedSite.id, cmd))())}
                      >
                        php artisan {cmd}
                      </button>
                    {/each}
                  </div>

                  {#if artisanOutput}
                    <div class="code-box">{`$ ${artisanOutput.command} (exit: ${artisanOutput.exitCode}, ${artisanOutput.durationMs}ms)\n\n${artisanOutput.stdout}${artisanOutput.stderr}`}</div>
                  {/if}
                </div>
              {:else if siteSubTab === 'workers'}
                <div class="card">
                  <div class="card-header">
                    <span class="card-title">
                      Background Workers per Situs (Supervised via Windows Job Object)
                    </span>
                  </div>
                  <p style="font-size:12px;color:var(--text-secondary);margin-bottom:12px;">
                    Setiap worker dijalankan dengan <code>CREATE_NO_WINDOW</code> dan auto-restart hingga 3× dalam 60 detik bila berhenti mendadak.
                  </p>

                  <div style="display:flex;flex-direction:column;gap:10px;">
                    {#each [
                      {
                        kind: 'queue' as WorkerKind,
                        title: 'Laravel Queue Worker',
                        cmd: 'php artisan queue:work --tries=3 --backoff=3',
                        desc: 'Memproses job asynchronous dari Redis atau database secara realtime.',
                      },
                      {
                        kind: 'scheduler' as WorkerKind,
                        title: 'Laravel Task Scheduler',
                        cmd: 'php artisan schedule:work',
                        desc: 'Menjalankan cron schedule Laravel setiap menit tanpa Task Scheduler Windows.',
                      },
                      {
                        kind: 'reverb' as WorkerKind,
                        title: 'Laravel Reverb WebSocket Server',
                        cmd: 'php artisan reverb:start',
                        desc: `Server WebSocket realtime yang di-proxy otomatis di reverb.${selectedSite.host}.`,
                      },
                    ] as w}
                      {@const active = selectedSite.workers.includes(w.kind)}
                      <div
                        style="display:flex;align-items:center;justify-content:space-between;padding:12px;border:1px solid var(--border);border-radius:6px;background:var(--bg-elevated);"
                      >
                        <div>
                          <div style="font-weight:600;display:flex;align-items:center;gap:8px;">
                            <span class="dot {active ? 'up' : 'off'}"></span>
                            <span>{w.title}</span>
                          </div>
                          <code style="font-size:11px;color:var(--text-secondary);">{w.cmd}</code>
                          <div style="font-size:11.5px;color:var(--text-secondary);margin-top:2px;">
                            {w.desc}
                          </div>
                        </div>
                        <button
                          class="btn {active ? 'btn-success' : ''}"
                          onclick={(event) => runButton(event, () => (() => handleToggleWorker(selectedSite.id, w.kind))())}
                        >
                          {active ? '● Running (Klik untuk Stop)' : '○ Off (Klik untuk Start)'}
                        </button>
                      </div>
                    {/each}
                  </div>
                </div>
              {:else if siteSubTab === 'env'}
                <div class="card">
                  <div class="card-header">
                    <div>
                      <span class="card-title">Editor .env Non-Destruktif</span>
                      <div style="font-size:11.5px;color:var(--text-secondary);">
                        Mempertahankan komentar asli; klik Autofill untuk menyuntikkan kredensial DB, Redis, SMTP (:1025), dan VarDumper (:9912).
                      </div>
                    </div>
                    <div style="display:flex;gap:8px;">
                      <button
                        class="btn btn-sm"
                        onclick={(event) => runButton(event, () => (() => handleAutofillSiteEnv(selectedSite.id))())}
                      >
                        ✨ Autofill Corral .env
                      </button>
                      <button
                        class="btn btn-primary btn-sm"
                        onclick={(event) => runButton(event, () => (() => handleSaveSiteEnv(selectedSite.id))())}
                      >
                        Simpan .env
                      </button>
                    </div>
                  </div>
                  <textarea
                    class="input"
                    style="height:310px;font-family:'Cascadia Code','Consolas',monospace;font-size:12px;line-height:1.5;"
                    bind:value={siteEnvContent}
                  ></textarea>
                </div>
              {:else if siteSubTab === 'workspaces'}
                <div class="card">
                  <div class="card-header">
                    <div>
                      <span class="card-title">Paralel Git Worktree Workspaces</span>
                      <div style="font-size:11.5px;color:var(--text-secondary);">
                        Buat subdomain terisolasi <code>&lt;branch&gt;.{selectedSite.host}</code> dengan database terpisah untuk QA paralel.
                      </div>
                    </div>
                    <div style="display:flex;gap:6px;">
                      <input
                        class="input"
                        style="width:210px;"
                        placeholder="Nama branch git..."
                        bind:value={newWorkspaceBranch}
                      />
                      <button
                        class="btn btn-primary btn-sm"
                        onclick={(event) => runButton(event, () => (() => handleCreateWorkspace(selectedSite.id))())}
                      >
                        + Buat Workspace
                      </button>
                    </div>
                  </div>

                  <table class="table">
                    <thead>
                      <tr>
                        <th>Branch</th>
                        <th>Domain Workspace</th>
                        <th>Database Terisolasi</th>
                        <th>Path Worktree</th>
                        <th>Aksi</th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each workspaces.filter((w) => w.siteId === selectedSite.id) as ws}
                        <tr>
                          <td><span class="pill">{ws.branch}</span></td>
                          <td><code>https://{ws.host}</code></td>
                          <td><code>{ws.dbName ?? '-'}</code></td>
                          <td style="font-size:11.5px;color:var(--text-secondary);">{ws.path}</td>
                          <td>
                            <button
                              class="btn btn-danger btn-sm"
                              onclick={(event) => runButton(event, () => (() => handleDeleteWorkspace(ws.id))())}
                            >
                              Hapus
                            </button>
                          </td>
                        </tr>
                      {:else}
                        <tr>
                          <td colspan="5" style="text-align:center;color:var(--text-secondary);padding:18px;">
                            Belum ada workspace aktif untuk situs ini.
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {/if}
            {/if}
          </div>
        </div>
      {:else if activeTab === 'services'}
        <!-- SERVICES, PHP.INI & DATABASES VIEW -->
        <div class="detail-column">
          <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;">
            <div>
              <h2 style="font-size:18px;font-weight:700;">Services, PHP FastCGI &amp; Databases</h2>
              <p style="font-size:12px;color:var(--text-secondary);">
                Semua proses anak diawasi oleh Windows Job Object (<code>JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE</code>) — 0 zombie process saat aplikasi ditutup.
              </p>
            </div>
            <div style="display:flex;gap:8px;">
              <button class="btn btn-primary" onclick={() => (showExtraServiceModal = true)}>
                + Extra Service Instance
              </button>
            </div>
          </div>

          <div class="subtabs">
            <button
              class="subtab-btn {serviceSubTab === 'daemons' ? 'active' : ''}"
              onclick={() => (serviceSubTab = 'daemons')}
            >
              Managed Services ({services.length})
            </button>
            <button
              class="subtab-btn {serviceSubTab === 'phpini' ? 'active' : ''}"
              onclick={(event) => runButton(event, () => (() => {
                serviceSubTab = 'phpini';
                return loadPhpIni(selectedPhpForIni);
              })())}
            >
              PHP Runtimes &amp; php.ini Editor
            </button>
            <button
              class="subtab-btn {serviceSubTab === 'databases' ? 'active' : ''}"
              onclick={() => (serviceSubTab = 'databases')}
            >
              MariaDB / MySQL Databases ({databases.length})
            </button>
          </div>

          {#if serviceSubTab === 'daemons'}
            <div class="card">
              <table class="table">
                <thead>
                  <tr>
                    <th>Service Name</th>
                    <th>Version</th>
                    <th>Port</th>
                    <th>Status</th>
                    <th>PID</th>
                    <th>Aksi</th>
                  </tr>
                </thead>
                <tbody>
                  {#each services as svc (svc.id)}
                    <tr>
                      <td style="font-weight:600;">{svc.name}</td>
                      <td><span class="pill">{svc.version}</span></td>
                      <td><code>127.0.0.1:{svc.port}</code></td>
                      <td>
                        <span class="pill {svc.state === 'running' ? 'pill-https' : ''}">
                          <span class="dot {svc.state === 'running' ? 'up' : 'off'}"></span>
                          {svc.state.toUpperCase()}
                        </span>
                      </td>
                      <td><code>{svc.pid ?? '-'}</code></td>
                      <td style="display:flex;gap:6px;">
                        {#if svc.state === 'running'}
                          <button
                            class="btn btn-sm"
                            onclick={(event) => runButton(event, () => (() => handleServiceAction(svc.id, 'restart'))())}
                          >
                            Restart
                          </button>
                          <button
                            class="btn btn-danger btn-sm"
                            onclick={(event) => runButton(event, () => (() => handleServiceAction(svc.id, 'stop'))())}
                          >
                            Stop
                          </button>
                        {:else}
                          <button
                            class="btn btn-primary btn-sm"
                            onclick={(event) => runButton(event, () => (() => handleServiceAction(svc.id, 'start'))())}
                          >
                            Start
                          </button>
                        {/if}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {:else if serviceSubTab === 'phpini'}
            <div class="card">
              <div class="card-header">
                <div style="display:flex;align-items:center;gap:10px;">
                  <span class="card-title">Pilih Versi PHP:</span>
                  {#each phpVersions as pv}
                    <button
                      class="btn btn-sm {selectedPhpForIni === pv.version ? 'btn-primary' : ''}"
                      onclick={(event) => runButton(event, () => (() => loadPhpIni(pv.version))())}
                    >
                      PHP {pv.version} {pv.activeGlobal ? '(Global)' : ''}
                    </button>
                  {/each}
                </div>
                <button class="btn btn-primary btn-sm" onclick={(event) => runButton(event, () => (handleSavePhpIni)())}>
                  Simpan &amp; Reload PHP {selectedPhpForIni}
                </button>
              </div>

              {#if phpIni}
                <div class="kv-grid">
                  <span class="kv-label">Lokasi php.ini</span>
                  <div style="display:flex;align-items:center;gap:8px;">
                    <code>{phpIni.rawPath}</code>
                    <button
                      class="btn btn-sm"
                      onclick={(event) => runButton(event, () => (() => call('open_path', { path: phpIni?.rawPath }))())}
                    >
                      Buka File
                    </button>
                  </div>

                  <span class="kv-label">memory_limit</span>
                  <input class="input" style="max-width:200px;" bind:value={phpIni.memoryLimit} />

                  <span class="kv-label">upload_max_filesize</span>
                  <input class="input" style="max-width:200px;" bind:value={phpIni.uploadMaxFilesize} />

                  <span class="kv-label">post_max_size</span>
                  <input class="input" style="max-width:200px;" bind:value={phpIni.postMaxSize} />

                  <span class="kv-label">max_execution_time</span>
                  <input class="input" style="max-width:200px;" bind:value={phpIni.maxExecutionTime} />

                  <span class="kv-label">realpath_cache_size (NTFS)</span>
                  <input class="input" style="max-width:200px;" bind:value={phpIni.realpathCacheSize} />

                  <span class="kv-label">OPcache + JIT Compiler</span>
                  <label style="display:flex;align-items:center;gap:8px;">
                    <input type="checkbox" bind:checked={phpIni.opcacheEnable} />
                    <span>Aktifkan Zend OPcache ({phpIni.opcacheMemoryConsumption} MB, JIT: {phpIni.opcacheJit})</span>
                  </label>

                  <span class="kv-label">Xdebug Mode</span>
                  <select class="select" style="max-width:200px;" bind:value={phpIni.xdebugMode}>
                    <option value="off">off (Zero Overhead - Recommended)</option>
                    <option value="debug">debug (Step Debugger port 9003)</option>
                    <option value="develop,debug">develop,debug</option>
                    <option value="profile">profile</option>
                  </select>
                </div>
              {/if}
            </div>
          {:else if serviceSubTab === 'databases'}
            <div class="card">
              <div class="card-header">
                <div>
                  <span class="card-title">Manajer Database Lokal (127.0.0.1:3306)</span>
                  {#if dbCredentials}
                    <div style="font-size:11.5px;color:var(--text-secondary);">
                      URL Koneksi: <code>{dbCredentials.connectionUrl}</code> (User: <code>{dbCredentials.username}</code>, Tanpa Password)
                    </div>
                  {/if}
                </div>
                <div style="display:flex;gap:6px;">
                  <input
                    class="input"
                    style="width:200px;"
                    placeholder="Nama database baru..."
                    bind:value={newDbName}
                  />
                  <button class="btn btn-primary btn-sm" onclick={(event) => runButton(event, () => (handleCreateDatabase)())}>
                    + Buat Database
                  </button>
                </div>
              </div>

              <table class="table">
                <thead>
                  <tr>
                    <th>Database Name</th>
                    <th>Engine</th>
                    <th>Tables</th>
                    <th>Ukuran</th>
                    <th>Situs Terkait</th>
                    <th>Aksi</th>
                  </tr>
                </thead>
                <tbody>
                  {#each databases as db (db.name)}
                    <tr>
                      <td><code>{db.name}</code></td>
                      <td>{db.engine}</td>
                      <td>{db.tablesCount} tabel</td>
                      <td>{(db.sizeKb / 1024).toFixed(2)} MB</td>
                      <td>{db.linkedSite ?? '-'}</td>
                      <td>
                        <button
                          class="btn btn-danger btn-sm"
                          onclick={(event) => runButton(event, () => (() => handleDropDatabase(db.name))())}
                        >
                          Drop
                        </button>
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        </div>
      {:else if activeTab === 'mail'}
        <!-- MAIL CATCHER SPLIT VIEW -->
        <div class="split-view">
          <div class="list-column">
            <div class="toolbar">
              <span style="font-weight:600;flex:1;">
                Inbox SMTP (:{mailSettings?.smtpPort ?? 1025})
              </span>
              <button class="btn btn-primary btn-sm" onclick={(event) => runButton(event, () => (handleSendTestMail)())}>
                + Kirim Email Uji
              </button>
              <button class="btn btn-sm" onclick={(event) => runButton(event, () => (handleClearMails)())}>Clear</button>
            </div>

            <div class="scroll-list">
              {#each mails as m (m.id)}
                <div
                  class="site-item {selectedMail?.id === m.id ? 'selected' : ''}"
                  role="button"
                  tabindex="0"
                  onclick={() => void selectMail(m.id)}
                  onkeydown={(e) => e.key === 'Enter' && void selectMail(m.id)}
                >
                  <div class="site-item-top">
                    <span style="font-weight:600;font-size:13px;">{m.subject}</span>
                  </div>
                  <div class="site-meta">
                    <span>→ {m.recipients.join(', ')}</span>
                    {#if m.attachments.length > 0}
                      <span class="pill">📎 {m.attachments.length}</span>
                    {/if}
                  </div>
                  <div style="font-size:11.5px;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                    {m.preview}
                  </div>
                </div>
              {:else}
                <div style="padding:28px;text-align:center;color:var(--text-secondary);">
                  Belum ada email masuk. Klik <strong>+ Kirim Email Uji</strong> atau panggil <code>Mail::to(...)-&gt;send()</code> di Laravel.
                </div>
              {/each}
            </div>
          </div>

          <div class="detail-column">
            {#if selectedMail}
              <div class="card">
                <div class="card-header">
                  <div>
                    <h3 style="font-size:16px;font-weight:700;">{selectedMail.subject}</h3>
                    <div style="font-size:12px;color:var(--text-secondary);margin-top:4px;">
                      From: <code>{selectedMail.envelopeFrom}</code> · To: <code>{selectedMail.recipients.join(', ')}</code>
                    </div>
                  </div>
                  <div style="display:flex;gap:6px;">
                    <button
                      class="btn btn-sm {mailPreviewMode === 'html' ? 'btn-primary' : ''}"
                      onclick={() => (mailPreviewMode = 'html')}
                    >
                      HTML Preview
                    </button>
                    <button
                      class="btn btn-sm {mailPreviewMode === 'text' ? 'btn-primary' : ''}"
                      onclick={() => (mailPreviewMode = 'text')}
                    >
                      Plain Text
                    </button>
                    <button
                      class="btn btn-sm {mailPreviewMode === 'headers' ? 'btn-primary' : ''}"
                      onclick={() => (mailPreviewMode = 'headers')}
                    >
                      Raw Headers
                    </button>
                  </div>
                </div>

                {#if loadingMailDetail && (!selectedMailDetail || selectedMailDetail.id !== selectedMail.id)}
                  <div class="code-box" style="color:var(--text-secondary);">
                    Memuat konten email...
                  </div>
                {:else if mailPreviewMode === 'html'}
                  <iframe
                    title="Sandboxed Email HTML Preview"
                    sandbox=""
                    srcdoc={selectedMailDetail?.id === selectedMail.id ? selectedMailDetail.htmlBody : ''}
                    style="width:100%;height:340px;border:1px solid var(--border);border-radius:6px;background:#ffffff;"
                  ></iframe>
                {:else if mailPreviewMode === 'text'}
                  <div class="code-box">{selectedMailDetail?.id === selectedMail.id ? selectedMailDetail.textBody : ''}</div>
                {:else}
                  <div class="code-box">{selectedMailDetail?.id === selectedMail.id ? selectedMailDetail.rawHeaders : ''}</div>
                {/if}

                {#if selectedMail.attachments.length > 0}
                  <div style="margin-top:12px;display:flex;gap:8px;align-items:center;">
                    <span style="font-size:12px;color:var(--text-secondary);">Attachments:</span>
                    {#each selectedMail.attachments as att}
                      <span class="pill">📎 {att.filename} ({(att.sizeBytes / 1024).toFixed(1)} KB)</span>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        </div>
      {:else if activeTab === 'dumps'}
        <!-- VARDUMPER RECEIVER VIEW -->
        <div class="detail-column">
          <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;">
            <div>
              <h2 style="font-size:18px;font-weight:700;">
                Symfony / Laravel VarDumper Server (tcp://127.0.0.1:9912)
              </h2>
              <p style="font-size:12px;color:var(--text-secondary);">
                Menangkap panggilan <code>dump($variable)</code> secara realtime tanpa mengotori response HTML/JSON aplikasi Anda.
              </p>
            </div>
            <div style="display:flex;gap:8px;">
              <button class="btn btn-primary" onclick={(event) => runButton(event, () => (handleSendTestDump)())}>
                + Simulasi dump($order)
              </button>
              <button
                class="btn"
                onclick={(event) => runButton(event, () => (async () => {
                  await call('clear_dumps');
                  await refreshAll();
                })())}
              >
                Clear All
              </button>
            </div>
          </div>

          {#each dumps as d (d.id)}
            <div class="card">
              <div class="card-header">
                <div style="display:flex;align-items:center;gap:8px;">
                  <span class="pill pill-laravel">{d.siteName ?? 'laravel.test'}</span>
                  <span class="pill">{d.kind}</span>
                  <code>{d.fileLine ?? ''}</code>
                </div>
              </div>
              <div class="code-box">{d.payload}</div>
            </div>
          {:else}
            <div class="card" style="text-align:center;color:var(--text-secondary);padding:32px;">
              Belum ada output <code>dump()</code> yang diterima.
            </div>
          {/each}
        </div>
      {:else if activeTab === 'logs'}
        <!-- REALTIME LOG VIEWER -->
        <div class="detail-column">
          <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;gap:10px;">
            <div style="display:flex;align-items:center;gap:8px;flex:1;">
              <select
                class="select"
                value={selectedLogPath}
                onchange={(e) => loadLogContent((e.currentTarget as HTMLSelectElement).value)}
              >
                {#each logFiles as lf}
                  <option value={lf.path}>{lf.label} ({lf.category})</option>
                {/each}
              </select>

              <select class="select" bind:value={logFilterLevel}>
                <option value="ALL">Semua Level</option>
                <option value="INFO">INFO</option>
                <option value="WARN">WARNING</option>
                <option value="ERR">ERROR</option>
              </select>

              <input
                class="input"
                style="max-width:240px;"
                placeholder="Filter teks log..."
                bind:value={logSearch}
              />
            </div>

            <div style="display:flex;gap:8px;">
              <button class="btn btn-sm" onclick={(event) => runButton(event, () => (() => loadLogContent(selectedLogPath))())}>
                ⟳ Refresh
              </button>
              <button class="btn btn-danger btn-sm" onclick={(event) => runButton(event, () => (handleClearLog)())}>
                Clear Log
              </button>
            </div>
          </div>

          <div class="code-box" style="flex:1;max-height:none;">
            {#each filteredLogLines as line (line.lineNumber)}
              <div style="margin-bottom:4px;">
                <span style="color:#6b7280;margin-right:8px;">#{line.lineNumber}</span>
                <span>{line.text}</span>
              </div>
            {:else}
              <div style="color:#9ca3af;">Tidak ada baris log yang cocok dengan filter.</div>
            {/each}
          </div>
        </div>
      {:else if activeTab === 'settings'}
        <!-- SETTINGS, SYSTEM TRAY & DOCTOR VIEW -->
        <div class="detail-column">
          {#if settings}
            <div class="card">
              <div class="card-header">
                <span class="card-title">Lokasi PHP</span>
                <button class="btn btn-primary btn-sm" onclick={(event) => runButton(event, () => (handleSaveSettings)())}>Simpan Pengaturan</button>
              </div>
              <p>Masukkan path folder PHP atau php.exe untuk setiap versi. Corral memakainya untuk membuat project dan menjalankan Artisan. Kosongkan untuk menggunakan runtime bawaan Corral.</p>
              <div class="kv-grid">
                {#each ['8.5', '8.4', '8.3', '8.2', '8.1'] as version}
                  <label class="kv-label" for="php-path-{version}">PHP {version}</label>
                  <input id="php-path-{version}" class="input" type="text" placeholder="C:\php\php-{version}\php.exe" bind:value={settings.phpPaths[version]} />
                {/each}
              </div>
            </div>
            <div class="card">
              <div class="card-header">
                <span class="card-title">Jendela, Tema &amp; System Tray Windows</span>
                <div style="display:flex;gap:8px;">
                  <button class="btn btn-sm" onclick={(event) => runButton(event, () => (handleMinimizeToTrayNow)())}>
                    ▾ Test Minimize to System Tray
                  </button>
                  <button class="btn btn-primary btn-sm" onclick={(event) => runButton(event, () => (handleSaveSettings)())}>
                    Simpan Pengaturan
                  </button>
                </div>
              </div>

              <div class="kv-grid">
                <span class="kv-label">Tema Tampilan</span>
                <select class="select" style="max-width:220px;" bind:value={settings.theme}>
                  <option value="system">Ikuti Sistem Windows</option>
                  <option value="dark">Dark Mode (Fluent Dark)</option>
                  <option value="light">Light Mode (Fluent Light)</option>
                </select>

                <span class="kv-label">Close to System Tray</span>
                <label style="display:flex;align-items:center;gap:8px;">
                  <input type="checkbox" bind:checked={settings.closeToTray} />
                  <span>Saat tombol Close (×) ditekan, sembunyikan Corral ke System Tray (server tetap hidup)</span>
                </label>

                <span class="kv-label">Minimize to System Tray</span>
                <label style="display:flex;align-items:center;gap:8px;">
                  <input type="checkbox" bind:checked={settings.minimizeToTray} />
                  <span>Saat tombol Minimize (_) ditekan, sembunyikan dari Taskbar ke System Tray</span>
                </label>

                <span class="kv-label">Autostart Saat Login</span>
                <label style="display:flex;align-items:center;gap:8px;">
                  <input type="checkbox" bind:checked={settings.autostartOnLogin} />
                  <span>Jalankan Corral secara otomatis saat masuk ke Windows</span>
                </label>

                <span class="kv-label">Domain TLD Default</span>
                <select class="select" style="max-width:220px;" bind:value={settings.defaultTld}>
                  <option value="test">.test (Rekomendasi RFC 6761)</option>
                  <option value="local">.local</option>
                  <option value="wip">.wip</option>
                  <option value="corral">.corral</option>
                </select>

                <span class="kv-label">Port HTTP / HTTPS</span>
                <div style="display:flex;gap:8px;align-items:center;">
                  <input class="input" type="number" style="width:95px;" bind:value={settings.httpPort} />
                  <span>/</span>
                  <input class="input" type="number" style="width:95px;" bind:value={settings.httpsPort} />
                </div>

                <span class="kv-label">Sertifikat Root CA</span>
                <div style="display:flex;gap:8px;align-items:center;">
                  {#if certStatus?.caTrusted}
                    <span class="pill pill-https">Trusted ✓</span>
                    <button class="btn btn-sm" onclick={(event) => runButton(event, () => (() => handleTrustCa(false))())}>
                      Revoke Trust
                    </button>
                  {:else}
                    <span class="pill">Belum di-trust</span>
                    <button class="btn btn-primary btn-sm" onclick={(event) => runButton(event, () => (() => handleTrustCa(true))())}>
                      Trust Corral Local CA
                    </button>
                  {/if}
                </div>
              </div>
            </div>
          {/if}

          <!-- CORRAL DOCTOR & PORT DIAGNOSTICS -->
          <div class="card">
            <div class="card-header">
              <span class="card-title">Corral Doctor &amp; Windows Port Conflict Inspector</span>
              <button class="btn btn-sm" onclick={(event) => runButton(event, () => (handleRunDoctor)())}>
                🩺 Jalankan Diagnostik Ulang
              </button>
            </div>

            {#if doctorReport}
              <div style="display:grid;grid-template-columns:1fr 1fr;gap:10px;margin-bottom:14px;">
                {#each doctorReport.checks as chk}
                  <div style="padding:10px;border:1px solid var(--border);border-radius:6px;background:var(--bg-elevated);">
                    <div style="font-weight:600;display:flex;align-items:center;gap:6px;">
                      <span class="dot {chk.status === 'ok' ? 'up' : 'starting'}"></span>
                      <span>{chk.title}</span>
                    </div>
                    <div style="font-size:12px;color:var(--text-secondary);margin-top:3px;">
                      {chk.detail}
                    </div>
                  </div>
                {/each}
              </div>

              <table class="table">
                <thead>
                  <tr>
                    <th>Port</th>
                    <th>Komponen</th>
                    <th>Status</th>
                    <th>Pemakai Port (PID)</th>
                    <th>Rekomendasi</th>
                  </tr>
                </thead>
                <tbody>
                  {#each doctorReport.portConflicts as pc}
                    <tr>
                      <td><code>:{pc.port}</code></td>
                      <td>{pc.serviceLabel}</td>
                      <td>
                        <span class="pill {!pc.inUse ? 'pill-https' : ''}">
                          {pc.inUse ? 'IN USE' : 'AVAILABLE'}
                        </span>
                      </td>
                      <td><code>{pc.processName ? `${pc.processName} (PID ${pc.pid})` : '-'}</code></td>
                      <td style="font-size:12px;color:var(--text-secondary);">{pc.recommendation}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            {/if}
          </div>
        </div>
      {/if}
    </main>
  </div>
</div>

<!-- MODAL: NEW LARAVEL SITE -->
{#if showNewSiteModal}
  <div class="modal-backdrop">
    <div class="modal-card">
      <h3 style="font-size:15px;font-weight:700;">Buat Situs Laravel Baru</h3>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Nama Proyek / Domain (.test)</span>
        <input class="input" placeholder="contoh: saas-billing" bind:value={newSiteName} />
      </label>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Template</span>
        <select class="select" bind:value={newSiteTemplate} disabled={creatingSite}>
          <option value="laravel">Laravel</option>
          <option value="livewire">Laravel + Livewire</option>
          <option value="react">Laravel + Inertia + React</option>
          <option value="vue">Laravel + Inertia + Vue.js</option>
        </select>
      </label>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Versi PHP FastCGI</span>
        <select class="select" bind:value={newSitePhp}>
          <option value="8.4">PHP 8.4 (Recommended - JIT Turbo)</option>
          <option value="8.5">PHP 8.5</option>
          <option value="8.3">PHP 8.3</option>
          <option value="8.2">PHP 8.2</option>
        </select>
      </label>
      <label style="display:flex;align-items:center;gap:8px;">
        <input type="checkbox" bind:checked={newSiteHttps} />
        <span>Aktifkan HTTPS otomatis (Sertifikat TLS lokal ≤ 396 hari)</span>
      </label>
      <div style="display:flex;justify-content:flex-end;gap:8px;margin-top:6px;">
        <button class="btn" disabled={creatingSite} onclick={() => (showNewSiteModal = false)}>Batal</button>
        <button class="btn btn-primary" disabled={creatingSite} onclick={(event) => runButton(event, () => (handleCreateSite)())}>{creatingSite ? 'Menginstal Laravel…' : 'Buat Situs'}</button>
      </div>
    </div>
  </div>
{/if}

<!-- MODAL: LINK EXISTING FOLDER -->
{#if showLinkFolderModal}
  <div class="modal-backdrop">
    <div class="modal-card">
      <h3 style="font-size:15px;font-weight:700;">Link Folder Proyek Laravel / PHP</h3>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Path Absolut Folder</span>
        <input class="input" placeholder="C:\Users\dev\Projects\my-laravel-app" bind:value={linkFolderPath} />
      </label>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Nama Domain (Opsional)</span>
        <input class="input" placeholder="my-laravel-app" bind:value={linkSiteName} />
      </label>
      <div style="display:flex;justify-content:flex-end;gap:8px;margin-top:6px;">
        <button class="btn" onclick={() => (showLinkFolderModal = false)}>Batal</button>
        <button class="btn btn-primary" onclick={(event) => runButton(event, () => (handleLinkFolder)())}>Link Folder</button>
      </div>
    </div>
  </div>
{/if}

<!-- MODAL: EXTRA SERVICE -->
{#if showExtraServiceModal}
  <div class="modal-backdrop">
    <div class="modal-card">
      <h3 style="font-size:15px;font-weight:700;">Tambah Instance Service Baru</h3>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Label Instance</span>
        <input class="input" bind:value={extraSvcName} />
      </label>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Jenis Engine</span>
        <select class="select" bind:value={extraSvcKind}>
          <option value="mariaDb">MariaDB / MySQL</option>
          <option value="postgres">PostgreSQL</option>
          <option value="redis">Redis</option>
          <option value="mongo">MongoDB</option>
        </select>
      </label>
      <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;">
        <label style="display:flex;flex-direction:column;gap:4px;">
          <span style="font-size:12px;color:var(--text-secondary);">Versi</span>
          <input class="input" bind:value={extraSvcVersion} />
        </label>
        <label style="display:flex;flex-direction:column;gap:4px;">
          <span style="font-size:12px;color:var(--text-secondary);">Port TCP</span>
          <input class="input" type="number" bind:value={extraSvcPort} />
        </label>
      </div>
      <div style="display:flex;justify-content:flex-end;gap:8px;margin-top:6px;">
        <button class="btn" onclick={() => (showExtraServiceModal = false)}>Batal</button>
        <button class="btn btn-primary" onclick={(event) => runButton(event, () => (handleCreateExtraService)())}>Tambah Service</button>
      </div>
    </div>
  </div>
{/if}

<!-- MODAL: CONFIRM DELETE / UNLINK SITE -->
{#if siteToDelete}
  <div
    class="modal-backdrop"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onclick={(e) => e.target === e.currentTarget && (siteToDelete = null)}
    onkeydown={(e) => e.key === 'Escape' && (siteToDelete = null)}
  >
    <div class="modal-card">
      <div style="display:flex;align-items:center;gap:10px;">
        <span style="display:inline-flex;align-items:center;justify-content:center;width:34px;height:34px;border-radius:8px;background:var(--danger-soft);color:var(--danger);font-size:17px;font-weight:700;">
          ⚠
        </span>
        <div>
          <h3 style="font-size:15px;font-weight:700;">Hapus Situs dari Corral?</h3>
          <span style="font-size:12px;color:var(--text-secondary);">Konfirmasi penghapusan virtual host</span>
        </div>
      </div>

      <p style="font-size:13px;line-height:1.5;color:var(--text-primary);">
        Apakah Anda yakin ingin menghapus situs <strong>{siteToDelete.host}</strong> dari daftar situs Corral?
      </p>

      <div style="background:var(--bg-elevated);border:1px solid var(--border);border-radius:6px;padding:10px 12px;">
        <div style="font-size:11.5px;color:var(--text-secondary);margin-bottom:3px;">Lokasi Folder:</div>
        <code style="font-size:12px;color:var(--text-primary);word-break:break-all;">{siteToDelete.path}</code>
        <div style="margin-top:8px;padding-top:6px;border-top:1px dashed var(--border);font-size:11.5px;color:var(--text-muted);display:flex;gap:6px;align-items:flex-start;">
          <span>ℹ</span>
          <span><strong>Aman:</strong> File source code proyek Anda tetap aman tersimpan di disk. Hanya routing Nginx dan entri daftar situs yang dihapus dari Corral.</span>
        </div>
      </div>

      <div style="display:flex;justify-content:flex-end;gap:8px;margin-top:4px;">
        <button class="btn" onclick={() => (siteToDelete = null)}>
          Batal
        </button>
        <button
          class="btn btn-danger"
          onclick={(event) => runButton(event, () => (async () => {
            if (siteToDelete) {
              const toDelete = siteToDelete;
              await handleDeleteSite(toDelete.id, toDelete.host);
              siteToDelete = null;
            }
          })())}
        >
          🗑 Hapus Situs
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- RIGHT-CLICK CONTEXT MENU FOR SITES -->
{#if siteContextMenu}
  <div
    class="context-menu"
    role="menu"
    tabindex="-1"
    style="left:{siteContextMenu.x}px;top:{siteContextMenu.y}px;"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.key === 'Escape' && closeSiteContextMenu()}
  >
    <div class="context-menu-header">
      {siteContextMenu.site.host}
    </div>
    <button
      class="context-menu-item"
      onclick={(event) => runButton(event, async () => {
        const id = siteContextMenu?.site.id;
        if (id) await call('open_in_browser', { siteId: id });
        closeSiteContextMenu();
      })}
    >
      <span>↗</span> Buka di Browser
    </button>
    <button
      class="context-menu-item"
      onclick={(event) => runButton(event, async () => {
        const id = siteContextMenu?.site.id;
        if (id) await call('open_in_editor', { siteId: id });
        closeSiteContextMenu();
      })}
    >
      <span>&lt;/&gt;</span> Buka di Editor
    </button>
    <button
      class="context-menu-item"
      onclick={(event) => runButton(event, async () => {
        const path = siteContextMenu?.site.path;
        if (path) await call('open_path', { path });
        closeSiteContextMenu();
      })}
    >
      <span>📁</span> Buka Folder di Explorer
    </button>
    <div class="context-menu-sep"></div>
    <button
      class="context-menu-item danger"
      onclick={() => {
        const site = siteContextMenu?.site;
        if (site) promptDeleteSite(site);
      }}
    >
      <span>🗑</span> Hapus Situs (Delete)...
    </button>
  </div>
{/if}

<svelte:window
  onclick={closeSiteContextMenu}
  onkeydown={(e) => {
    if (e.key === 'Escape') {
      closeSiteContextMenu();
      siteToDelete = null;
    }
  }}
/>

{#if notifications.length > 0}
  <div class="modal-backdrop notification-backdrop">
    <div class="modal-card notification-card" role="alertdialog" tabindex="-1" aria-modal="true" aria-labelledby="notification-title" aria-describedby="notification-message" use:focusNotification onkeydown={(event) => { if (event.key === 'Escape') { event.stopPropagation(); dismissNotification(); } else if (event.key === 'Tab') { event.preventDefault(); } }}>
      <div class="notification-heading">
        <span class="notification-icon {notifications[0].kind}" aria-hidden="true">{notifications[0].kind === 'error' ? '!' : notifications[0].kind === 'info' ? 'i' : '✓'}</span>
        <h3 id="notification-title">{notifications[0].kind === 'error' ? 'Terjadi kesalahan' : notifications[0].kind === 'info' ? 'Informasi' : 'Berhasil'}</h3>
      </div>
      <p id="notification-message">{notifications[0].message}</p>
      <div class="notification-actions"><button class="btn btn-primary" onclick={dismissNotification}>Tutup</button></div>
    </div>
  </div>
{/if}
