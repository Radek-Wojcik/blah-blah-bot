import { endRun } from "./tmp";

export default function globalTeardown(): void {
  endRun();
}
