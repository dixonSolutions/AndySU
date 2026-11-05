import { NativeEventEmitter, NativeModules, Platform } from 'react-native';

/** Update status as reported by the AndySU host. */
export interface Status {
  state: 'idle' | 'checking' | 'up_to_date' | 'deferred' | 'available' | 'downloading' | 'staged' |
    'applying' | 'shell_update' | 'error';
  release?: string;
  hot_only?: boolean;
  mandatory?: boolean;
  download_bytes?: number;
  code?: string;
  message?: string;
  [k: string]: unknown;
}

export interface HostInfo {
  /** False when the app is not running under an AndySU host (e.g. a debug build). */
  present: boolean;
  apiLevel?: number;
  version?: string;
  isFactory?: boolean;
  slotDir?: string;
}

const native = Platform.OS === 'android' ? NativeModules.AndySU : undefined;

/** Information about the running payload. */
export async function info(): Promise<HostInfo> {
  if (!native) return { present: false };
  return JSON.parse(await native.info());
}

/** The updater's current status. */
export async function status(): Promise<Status> {
  if (!native) return { state: 'idle' };
  return JSON.parse(await native.status());
}

/** Check now (download and stage according to the configured mode). */
export function checkNow(): void {
  native?.checkNow();
}

/** Apply a staged update now (live if possible, otherwise by restarting). */
export function applyNow(): void {
  native?.applyNow();
}

/** Restart into the newest staged or committed payload. */
export function restartNow(): void {
  native?.restartNow();
}

export type ApplyPolicy = 'live' | 'restart' | 'next_start';

/**
 * How staged updates apply on this install: swap live (the default), restart the app, or wait for
 * the next start. Pass null to return to the policy configured in andysu.toml.
 */
export function setApplyPolicy(policy: ApplyPolicy | null): void {
  native?.setApplyPolicy(policy);
}

export async function applyPolicy(): Promise<ApplyPolicy> {
  if (!native) return 'live';
  return native.applyPolicy();
}

export type UpdateMode = 'auto' | 'download_only' | 'check_only' | 'off';

/**
 * How much automatic updating this install performs: 'auto', 'download_only', 'check_only' or
 * 'off'. Pass null to return to the mode configured in the host. Needs Host API level 3; on an
 * older host this does nothing.
 */
export function setUpdateMode(mode: UpdateMode | null): void {
  native?.setUpdateMode(mode);
}

/** The effective update mode, or null when the host is absent or older than API level 3. */
export async function updateMode(): Promise<UpdateMode | null> {
  if (!native) return null;
  const m: string = await native.updateMode();
  return m ? (m as UpdateMode) : null;
}

/** Commit this payload generation (call once the app is known to work). */
export function markHealthy(): void {
  native?.markHealthy();
}

/** Listen for status changes. Returns an unsubscribe function. */
export function onStatus(cb: (s: Status) => void): () => void {
  if (!native) return () => {};
  const sub = new NativeEventEmitter(native).addListener('AndySUStatus', (json: string) => cb(JSON.parse(json)));
  native.listen();
  return () => sub.remove();
}
