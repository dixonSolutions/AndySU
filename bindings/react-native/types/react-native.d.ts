// The slice of react-native this package uses, so it type-checks without installing React Native.
declare module 'react-native' {
  export const NativeModules: { [name: string]: any };
  export const Platform: { OS: string };
  export class NativeEventEmitter {
    constructor(module?: any);
    addListener(event: string, cb: (data: any) => void): { remove(): void };
  }
}
