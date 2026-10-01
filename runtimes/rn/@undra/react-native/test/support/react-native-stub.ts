// The React Native APIs @undra/react-native uses, as a scriptable stub for tests on Node.

/** A TurboModule spec's base interface. */
export interface TurboModule {
  readonly getConstants?: () => object;
}

/** The app's state as `AppState` reports it. */
export type AppStateStatus = "active" | "background" | "inactive" | "unknown" | "extension";

const modules = new Map<string, unknown>();

/** Registers what `TurboModuleRegistry.get(name)` returns. */
export function setTurboModule(name: string, module: unknown): void {
  modules.set(name, module);
}

export const TurboModuleRegistry = {
  get<T extends TurboModule>(name: string): T | null {
    return (modules.get(name) as T | undefined) ?? null;
  },
  getEnforcing<T extends TurboModule>(name: string): T {
    const module = modules.get(name);
    if (module === undefined) throw new Error(`TurboModule ${name} is not registered`);
    return module as T;
  },
};

const listeners = new Set<(state: AppStateStatus) => void>();
let state: AppStateStatus = "active";

/** Moves the stub app to `next` and tells the listeners. */
export function setAppState(next: AppStateStatus): void {
  state = next;
  for (const listener of [...listeners]) listener(next);
}

export const AppState = {
  get currentState(): AppStateStatus {
    return state;
  },
  addEventListener(_type: "change", listener: (state: AppStateStatus) => void): { remove(): void } {
    listeners.add(listener);
    return {
      remove() {
        listeners.delete(listener);
      },
    };
  },
};

export const Platform = { OS: "test" };
