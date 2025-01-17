import van from "vanjs-core";

const vscodeAPI = typeof acquireVsCodeApi !== "undefined" && acquireVsCodeApi();

export const traceData = van.state<string | undefined>(undefined);

// todo
// {
//     "command": "tinymist.traceCurrentFile",
//     "title": "Trace and visualize execution of the current Typst file",
//     "when": "editorLangId == disabled",
//     "category": "Typst"
// }

// panel.webview.postMessage({ type: "traceData", data: Mock });

/// A frontend will try to setup a vscode channel if it is running
/// in vscode.
export function setupVscodeChannel() {
  if (vscodeAPI?.postMessage) {
    // Handle messages sent from the extension to the webview
    window.addEventListener("message", (event: any) => {
      switch (event.data.type) {
        case "traceData": {
          traceData.val = event.data.data;
          break;
        }
      }
    });
  }
}

export function requestSavePackageData(data: any) {
  if (vscodeAPI?.postMessage) {
    vscodeAPI.postMessage({ type: "savePackageData", data });
  }
}

export function requestInitTemplate(packageSpec: string) {
  if (vscodeAPI?.postMessage) {
    vscodeAPI.postMessage({ type: "initTemplate", packageSpec });
  }
}

export function requestRevealPath(path: string) {
  if (vscodeAPI?.postMessage) {
    vscodeAPI.postMessage({ type: "revealPath", path });
  }
}
