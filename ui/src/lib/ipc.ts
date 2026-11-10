import { invoke } from '@tauri-apps/api/core';
import { parseAppError } from './errors';

export type WorkerKind = 'queue' | 'scheduler' | 'reverb';

export interface AppInfoView {
  name: string;
  version: string;
  platform: string;
  arch: string;
  dataDir: string;
  localDataDir: string;
  helperInstalled: boolean;
  caTrusted: boolean;
  defaultPhpVersion: string;
  defaultNodeVersion: string;
  closeToTray: boolean;
  minimizeToTray: boolean;
}

export interface SiteView {
  id: string;
  name: string;
  host: string;
  hostnames: string[];
  path: string;
  documentRoot: string;
  phpVersion: string;
  nodeVersion: string;
  https: boolean;
  framework: string;
  frameworkBadge: string;
  isLaravel: boolean;
  healthy: boolean;
  dbName: string | null;
  workers: WorkerKind[];
  opcacheEnabled: boolean;
  xdebugEnabled: boolean;
  hasCorralYml: boolean;
  url: string;
}

export interface PhpExtensionView {
  name: string;
  enabled: boolean;
  available: boolean;
  description: string;
}

export interface PhpVersionView {
  version: string;
  installed: boolean;
  activeGlobal: boolean;
  fastcgiPort: number;
  running: boolean;
  binaryPath: string;
  iniPath: string;
  extensions: PhpExtensionView[];
  opcacheTurbo: boolean;
  xdebugEnabled: boolean;
}

export interface PhpIniSettingsDto {
  version: string;
  memoryLimit: string;
  uploadMaxFilesize: string;
  postMaxSize: string;
  maxExecutionTime: string;
  maxInputVars: string;
  realpathCacheSize: string;
  realpathCacheTtl: string;
  opcacheEnable: boolean;
  opcacheMemoryConsumption: string;
  opcacheMaxAcceleratedFiles: string;
  opcacheJit: string;
  opcacheJitBufferSize: string;
  xdebugMode: string;
  xdebugClientPort: number;
  rawPath: string;
}

export interface NodeVersionView {
  version: string;
  ltsName: string;
  installed: boolean;
  activeGlobal: boolean;
  composerInstalled: boolean;
  laravelInstallerInstalled: boolean;
}

export interface ServiceView {
  id: string;
  name: string;
  kind: string;
  version: string;
  port: number;
  pid: number | null;
  state: string;
  stateDetail: string | null;
  autostart: boolean;
  dataDir: string | null;
  siteId: string | null;
}

export interface DatabaseView {
  name: string;
  engine: string;
  port: number;
  tablesCount: number;
  sizeKb: number;
  linkedSite: string | null;
}

export interface DbCredentialsView {
  host: string;
  port: number;
  username: string;
  password: string;
  defaultDatabase: string;
  connectionUrl: string;
}

export interface CertStatusView {
  caExists: boolean;
  caTrusted: boolean;
  caSubject: string;
  caThumbprint: string;
  caNotAfter: string;
  caPath: string;
  activeSiteCerts: number;
}

export interface LogFileView {
  id: string;
  label: string;
  category: string;
  path: string;
  sizeBytes: number;
}

export interface LogLine {
  lineNumber: number;
  timestamp: string | null;
  level: string;
  text: string;
}

export interface MailAttachment {
  filename: string;
  contentType: string;
  sizeBytes: number;
}

export interface MailMessage {
  id: string;
  receivedAt: number;
  envelopeFrom: string;
  recipients: string[];
  subject: string;
  preview: string;
  htmlBody: string;
  textBody: string;
  rawHeaders: string;
  attachments: MailAttachment[];
  read: boolean;
}

export interface MailSettingsView {
  smtpHost: string;
  smtpPort: number;
  running: boolean;
  totalCaptured: number;
  unreadCount: number;
  envSnippet: string;
}

export interface DumpEntry {
  id: string;
  timestamp: number;
  siteName: string | null;
  fileLine: string | null;
  kind: string;
  payload: string;
}

export interface WorkspaceView {
  id: string;
  siteId: string;
  siteName: string;
  branch: string;
  path: string;
  host: string;
  dbName: string | null;
  createdAt: number;
}

export interface HelperStatusView {
  installed: boolean;
  running: boolean;
  pipeName: string;
  mode: string;
  hostsWritable: boolean;
  message: string;
}

export interface PortConflictView {
  port: number;
  serviceLabel: string;
  inUse: boolean;
  pid: number | null;
  processName: string | null;
  excludedByWinNat: boolean;
  recommendation: string;
}

export interface DoctorCheckView {
  id: string;
  title: string;
  status: string;
  detail: string;
  fixHint: string | null;
}

export interface DoctorReportView {
  overallHealthy: boolean;
  checks: DoctorCheckView[];
  portConflicts: PortConflictView[];
}

export interface LaravelOptimizationItem {
  key: string;
  label: string;
  category: string;
  currentValue: string;
  recommendedValue: string;
  optimized: boolean;
  impact: string;
}

export interface LaravelHealthReport {
  siteId: string;
  siteName: string;
  isLaravel: boolean;
  laravelVersion: string;
  score: number;
  opcacheTurbo: boolean;
  realpathCacheTuned: boolean;
  fastcgiPoolChildren: number;
  envConfigured: boolean;
  storageLinked: boolean;
  items: LaravelOptimizationItem[];
  artisanCommands: string[];
}

export interface ArtisanOutputDto {
  command: string;
  exitCode: number;
  stdout: string;
  stderr: string;
  durationMs: number;
}

export interface AppSettingsDto {
  theme: string;
  defaultPhpVersion: string;
  defaultNodeVersion: string;
  defaultTld: string;
  httpPort: number;
  httpsPort: number;
  basePhpPort: number;
  smtpPort: number;
  dumpsPort: number;
  parkedPaths: string[];
  closeToTray: boolean;
  minimizeToTray: boolean;
  startMinimized: boolean;
  autostartOnLogin: boolean;
  micaEffect: boolean;
  autoStartServices: boolean;
  autoConfigureLaravelEnv: boolean;
  laravelOpcacheTurbo: boolean;
  codeEditor: string;
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw parseAppError(e);
  }
}
