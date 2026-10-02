import { Component, type ReactNode } from "react";
import { detailsApi } from "../ipc.js";
import { currentMountGeneration, failureText, reportRendererError } from "./renderer-error-listeners.js";

interface Props { readonly children: ReactNode }
interface State { readonly failed: boolean }

export class DetailsErrorBoundary extends Component<Props, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: Error): void {
    reportRendererError(error);
    void currentMountGeneration().then((generation) => detailsApi.ready(generation, true));
  }

  render(): ReactNode {
    if (this.state.failed) {
      const { heading, reload } = failureText();
      return <main className="details-shell" role="alert">
        <h1>{heading}</h1>
        <button type="button" onClick={() => window.location.reload()}>{reload}</button>
      </main>;
    }
    return this.props.children;
  }
}
