// Solid context provider for the app state machine
import { createContext, useContext, type ParentComponent } from "solid-js";
import type { AppMachine } from "./machine";
import { createAppMachine } from "./machine";

const AppContext = createContext<AppMachine>();

export const AppProvider: ParentComponent = (props) => {
  const machine = createAppMachine();
  return <AppContext.Provider value={machine}>{props.children}</AppContext.Provider>;
};

export function useAppState(): AppMachine {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useAppState must be used within AppProvider");
  return ctx;
}
