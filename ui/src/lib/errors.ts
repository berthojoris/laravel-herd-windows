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

export function parseAppError(raw: unknown): AppErrorKind {
  if (raw && typeof raw === 'object' && 'kind' in raw && 'message' in raw) {
    const obj = raw as { kind: string; message: string };
    return {
      kind: obj.kind as AppErrorKind['kind'],
      message: String(obj.message),
    };
  }
  if (typeof raw === 'string') {
    try {
      const parsed = JSON.parse(raw) as unknown;
      if (parsed && typeof parsed === 'object' && 'kind' in parsed && 'message' in parsed) {
        return parsed as AppErrorKind;
      }
    } catch {
      // Plain text error
    }
    return { kind: 'io', message: raw };
  }
  if (raw instanceof Error) {
    return { kind: 'io', message: raw.message };
  }
  return { kind: 'io', message: 'Terjadi kesalahan tidak terduga pada sistem Corral.' };
}

export function messageFor(err: AppErrorKind): string {
  return err.message;
}
