import { useEffect, useRef } from "react";
import type { EngineEvent } from "@axonmind/types";
import { useAxonMind } from "../context";

/** Subscribe to engine events. `handler` is stabilised via ref — no need to memoize. */
export function useEngineEvents(handler: (event: EngineEvent) => void): void {
  const { transport } = useAxonMind();
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    if (!transport.onEvent) return;
    return transport.onEvent(event => handlerRef.current(event));
  }, [transport]);
}
