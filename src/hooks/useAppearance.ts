import { useLayoutEffect } from "react";
import type { Appearance } from "../types/settings";

export function useAppearance(appearance: Appearance = "system") {
  useLayoutEffect(() => {
    const root = document.documentElement;
    if (appearance !== "system") {
      root.dataset.appearance = appearance;
      return () => { delete root.dataset.appearance; };
    }

    const preference = window.matchMedia("(prefers-color-scheme: dark)");
    function update() {
      root.dataset.appearance = preference.matches ? "dark" : "light";
    }
    update();
    preference.addEventListener("change", update);
    return () => {
      preference.removeEventListener("change", update);
      delete root.dataset.appearance;
    };
  }, [appearance]);
}
