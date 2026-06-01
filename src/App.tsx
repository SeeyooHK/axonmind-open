import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TauriTransport, AxonMindProvider } from "@axonmind/react";
import { AppShell } from "./AppShell";

const transport = new TauriTransport(invoke, listen);

export default function App() {
  return (
    <AxonMindProvider transport={transport}>
      <AppShell />
    </AxonMindProvider>
  );
}
