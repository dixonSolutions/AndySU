// The slice of @capacitor/core this package uses.
declare module '@capacitor/core' {
  export function registerPlugin<T>(name: string, impl?: unknown): T;
  export interface PluginListenerHandle { remove(): Promise<void>; }
}
