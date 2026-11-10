import Foundation

/// Small in-memory event log (last 50 lines) shown on the log sheet. Never put the pairing secret here.
final class DebugLog: ObservableObject {
    static let shared = DebugLog()
    @Published private(set) var lines: [String] = []
    private static let fmt: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss.SSS"
        return f
    }()

    static func log(_ s: String) { shared.add(s) }

    func add(_ s: String) {
        let now = Date()
        DispatchQueue.main.async {
            self.lines.append(DebugLog.fmt.string(from: now) + " " + s)
            if self.lines.count > 50 { self.lines.removeFirst(self.lines.count - 50) }
        }
    }
}
