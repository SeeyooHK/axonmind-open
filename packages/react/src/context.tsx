import { createContext, useContext, type ReactNode } from "react";
import type { AxonMindTransport } from "@axonmind/types";

interface AxonMindContextValue {
  transport: AxonMindTransport;
}

const AxonMindContext = createContext<AxonMindContextValue | null>(null);

export interface AxonMindProviderProps {
  transport: AxonMindTransport;
  children: ReactNode;
}

export function AxonMindProvider({ transport, children }: AxonMindProviderProps) {
  return (
    <AxonMindContext.Provider value={{ transport }}>
      {children}
    </AxonMindContext.Provider>
  );
}

export function useAxonMind(): AxonMindContextValue {
  const ctx = useContext(AxonMindContext);
  if (!ctx) throw new Error("useAxonMind must be used inside <AxonMindProvider>");
  return ctx;
}
