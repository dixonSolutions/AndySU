import { registerPlugin, PluginListenerHandle } from '@capacitor/core';

export type ApplyPolicy = 'live' | 'restart' | 'next_start';

export interface Status { state: string; release?: string; [k: string]: unknown }

export interface AndySUPlugin {
  info(): Promise<{ present: boolean; apiLevel?: number; version?: string; isFactory?: boolean; slotDir?: string }>;
  status(): Promise<Status>;
  checkNow(): Promise<void>;
  applyNow(): Promise<void>;
  restartNow(): Promise<void>;
  markHealthy(): Promise<void>;
  setApplyPolicy(o: { policy: ApplyPolicy | null }): Promise<void>;
  applyPolicy(): Promise<{ policy: ApplyPolicy }>;
  addListener(event: 'status', cb: (s: Status) => void): Promise<PluginListenerHandle>;
}

/** On platforms without AndySU every call resolves with neutral values. */
const web: AndySUPlugin = {
  info: async () => ({ present: false }),
  status: async () => ({ state: 'idle' }),
  checkNow: async () => {},
  applyNow: async () => {},
  restartNow: async () => {},
  markHealthy: async () => {},
  setApplyPolicy: async () => {},
  applyPolicy: async () => ({ policy: 'live' }),
  addListener: async () => ({ remove: async () => {} }),
};

export const AndySU = registerPlugin<AndySUPlugin>('AndySU', { web });
