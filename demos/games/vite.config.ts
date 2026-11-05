import { defineConfig } from "vite";
import { demoConfig } from "../vite-config.mts";

export default defineConfig(({ command }) => demoConfig(import.meta.dirname, command));
