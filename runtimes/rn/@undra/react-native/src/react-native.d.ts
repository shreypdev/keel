// The few React Native APIs this package uses, typed for its own build and tests. An app compiles
// against React Native's real types; this file is never imported, so it is not part of what an app
// type-checks.
declare module "react-native" {
  /** A TurboModule spec's base interface. */
  export interface TurboModule {
    readonly getConstants?: () => object;
  }
  /** Where TurboModules are looked up by name. */
  export const TurboModuleRegistry: {
    get<T extends TurboModule>(name: string): T | null | undefined;
    getEnforcing<T extends TurboModule>(name: string): T;
  };
  /** The app's state as `AppState` reports it. */
  export type AppStateStatus = "active" | "background" | "inactive" | "unknown" | "extension";
  /** The app's foreground/background state. */
  export const AppState: {
    readonly currentState: AppStateStatus | null | undefined;
    addEventListener(type: "change", listener: (state: AppStateStatus) => void): { remove(): void };
  };
  /** The platform the app runs on. */
  export const Platform: { readonly OS: string };
}
