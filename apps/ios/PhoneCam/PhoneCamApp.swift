import SwiftUI
import AVFoundation

@main
struct PhoneCamApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                .preferredColorScheme(.dark)
                .onAppear { model.start() }
        }
    }
}

struct PreviewView: UIViewRepresentable {
    let session: AVCaptureSession

    final class Host: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
    }

    func makeUIView(context: Context) -> Host {
        let v = Host()
        v.previewLayer.session = session
        v.previewLayer.videoGravity = .resizeAspect
        v.backgroundColor = .black
        return v
    }

    func updateUIView(_ uiView: Host, context: Context) {}
}

struct ContentView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            PreviewView(session: model.camera.session).ignoresSafeArea()
            VStack {
                Spacer()
                Text(model.status)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundColor(.white.opacity(0.85))
                    .padding(.horizontal, 12).padding(.vertical, 6)
                    .background(Color.black.opacity(0.55))
                    .padding(.bottom, 10)
                Button(action: { model.showScanner = true }) {
                    Text(model.paired ? "Re-pair" : "Pair")
                        .font(.system(.subheadline, design: .monospaced))
                        .foregroundColor(.white)
                        .padding(.horizontal, 28).padding(.vertical, 10)
                        .overlay(Rectangle().stroke(Color.white.opacity(0.5), lineWidth: 1))
                        .background(Color.black.opacity(0.55))
                }
                .padding(.bottom, 8)
                HStack(spacing: 16) {
                    Text("v\(AppModel.version)")
                    Button("Log") { model.showLog = true }
                }
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(.white.opacity(0.6))
                .padding(.bottom, 16)
            }
        }
        .sheet(isPresented: $model.showLog) { LogView() }
        .sheet(isPresented: $model.showScanner, onDismiss: { model.resumeCamera() }) {
            ZStack(alignment: .top) {
                ScannerView { code in
                    if model.handleScanned(code) { model.showScanner = false }
                    else { model.pairError = "Not a PhoneCam code" }
                }
                .ignoresSafeArea()
                Text(model.pairError ?? "Scan the QR on your PC")
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundColor(.white)
                    .padding(8).background(Color.black.opacity(0.6))
                    .padding(.top, 24)
            }
            .onAppear { model.pairError = nil; model.suspendCamera() }
        }
    }
}

struct LogView: View {
    @ObservedObject private var log = DebugLog.shared
    @Environment(\.presentationMode) private var presentation

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("PhoneCam v\(AppModel.version)")
                    Spacer()
                    Button("Close") { presentation.wrappedValue.dismiss() }
                }
                .font(.system(.footnote, design: .monospaced))
                .foregroundColor(.white)
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(Array(log.lines.enumerated()), id: \.offset) { _, line in
                            Text(line)
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundColor(.white.opacity(0.8))
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
            }
            .padding(12)
        }
        .preferredColorScheme(.dark)
    }
}
