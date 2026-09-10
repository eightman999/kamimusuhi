import Foundation
import Darwin
import IOKit.ps

// Only numerical machine telemetry and stable sensor provenance leave this process.
// Peer and output path are command arguments only; neither is copied into records.
struct Options {
    var duration: Double = 60
    var interval: Double = 1
    var samples: Int? = nil
    var peerHost: String? = nil
    var output: String? = nil
}

func options() -> Options {
    var result = Options()
    let args = Array(CommandLine.arguments.dropFirst())
    if args.contains("--help") {
        print("k-sense-mac --duration SECONDS --interval SECONDS [--samples N] [--peer-host HOST] [--output PATH]\nRead-only macOS machine telemetry JSONL; default duration 60 s, interval 1 s. No host, IP, path or process text is saved.")
        exit(0)
    }
    guard args.count % 2 == 0 else { fail("Options require values") }
    for index in stride(from: 0, to: args.count, by: 2) {
        switch args[index] {
        case "--duration": result.duration = Double(args[index + 1]) ?? -1
        case "--interval": result.interval = Double(args[index + 1]) ?? -1
        case "--samples": result.samples = Int(args[index + 1]) ?? -1
        case "--peer-host": result.peerHost = args[index + 1]
        case "--output": result.output = args[index + 1]
        default: fail("Unsupported option")
        }
    }
    guard result.duration.isFinite, result.duration > 0, result.interval.isFinite, result.interval >= 0.5,
          result.samples == nil || result.samples! > 0 else { fail("Duration/samples must be positive; interval must be at least 0.5 s") }
    return result
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(2)
}

func nullOrNumber(_ value: Double?) -> Any {
    if let value = value, value.isFinite { return value }
    return NSNull()
}

func cpuTicks() -> [UInt32]? {
    var value = host_cpu_load_info()
    var count = mach_msg_type_number_t(MemoryLayout<host_cpu_load_info>.size / MemoryLayout<integer_t>.size)
    let code = withUnsafeMutablePointer(to: &value) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, $0, &count) }
    }
    guard code == KERN_SUCCESS else { return nil }
    return [value.cpu_ticks.0, value.cpu_ticks.1, value.cpu_ticks.2, value.cpu_ticks.3]
}

func cpuFraction(_ previous: [UInt32]?, _ current: [UInt32]?) -> Double? {
    guard let previous = previous, let current = current, previous.count == current.count else { return nil }
    let delta = zip(current, previous).map { Double($0 &- $1) }
    let total = delta.reduce(0, +)
    guard total > 0 else { return nil }
    return max(0, min(1, 1 - delta[Int(CPU_STATE_IDLE)] / total))
}

func memoryMetrics() -> [String: Double?] {
    let total = Double(ProcessInfo.processInfo.physicalMemory)
    var stats = vm_statistics64()
    var count = mach_msg_type_number_t(MemoryLayout<vm_statistics64>.size / MemoryLayout<integer_t>.size)
    let code = withUnsafeMutablePointer(to: &stats) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &count) }
    }
    var result: [String: Double?] = ["memory_total_bytes": total, "memory_available_bytes": nil, "memory_pressure": nil, "swap_total_bytes": nil, "swap_used_bytes": nil]
    if code == KERN_SUCCESS {
        // active+wired+compressor is a load proxy, not macOS dispatch memory-pressure state.
        let page = Double(vm_kernel_page_size)
        let occupied = min(total, (Double(stats.active_count) + Double(stats.wire_count) + Double(stats.compressor_page_count)) * page)
        result["memory_available_bytes"] = total - occupied
        result["memory_pressure"] = occupied / total
    }
    var swap = xsw_usage()
    var size = MemoryLayout<xsw_usage>.size
    if sysctlbyname("vm.swapusage", &swap, &size, nil, 0) == 0 {
        result["swap_total_bytes"] = Double(swap.xsu_total)
        result["swap_used_bytes"] = Double(swap.xsu_used)
    }
    return result
}

func powerMetrics() -> [String: Double?] {
    var result: [String: Double?] = ["battery_fraction": nil, "battery_charging": nil, "ac_power": nil, "power_pressure": nil]
    guard let information = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
          let sources = IOPSCopyPowerSourcesList(information)?.takeRetainedValue() as? [CFTypeRef] else { return result }
    // Power-source type works for AC-only desktop machines as well.
    if let providing = IOPSGetProvidingPowerSourceType(information)?.takeUnretainedValue() {
        let ac = (providing as String) == kIOPSACPowerValue
        result["ac_power"] = ac ? 1 : 0
        if ac { result["power_pressure"] = 0 }
    }
    for source in sources {
        guard let description = IOPSGetPowerSourceDescription(information, source)?.takeUnretainedValue() as? [String: Any],
              let current = description[kIOPSCurrentCapacityKey] as? NSNumber,
              let maximum = description[kIOPSMaxCapacityKey] as? NSNumber, maximum.doubleValue > 0 else { continue }
        let battery = max(0, min(1, current.doubleValue / maximum.doubleValue))
        result["battery_fraction"] = battery
        if let charging = description[kIOPSIsChargingKey] as? NSNumber {
            result["battery_charging"] = charging.boolValue ? 1 : 0
        }
        if let power = description[kIOPSPowerSourceStateKey] as? String {
            let ac = power == kIOPSACPowerValue
            result["ac_power"] = ac ? 1 : 0
            result["power_pressure"] = ac ? 0 : 1 - battery
        }
        break
    }
    return result
}

func residentBytes() -> Double? {
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
    let code = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count) }
    }
    return code == KERN_SUCCESS ? Double(info.resident_size) : nil
}

func consumedCPU() -> Double {
    var usage = rusage()
    getrusage(RUSAGE_SELF, &usage)
    let own = Double(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) + Double(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) / 1e6
    getrusage(RUSAGE_CHILDREN, &usage)
    return own + Double(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) + Double(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) / 1e6
}

func peerMetrics(_ peer: String?) -> [String: Double?] {
    guard let peer = peer else { return ["network_rtt_ms": nil, "network_loss": nil, "network_connectivity": nil] }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/sbin/ping")
    process.arguments = ["-n", "-c", "1", "-W", "500", "--", peer]
    let output = Pipe()
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    do { try process.run() } catch { return ["network_rtt_ms": nil, "network_loss": 1, "network_connectivity": 0] }
    let deadline = ProcessInfo.processInfo.systemUptime + 0.7
    while process.isRunning && ProcessInfo.processInfo.systemUptime < deadline { usleep(10000) }
    if process.isRunning { process.terminate() }
    process.waitUntilExit()
    let text = String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    var rtt: Double? = nil
    if let regex = try? NSRegularExpression(pattern: "time[=<]([0-9.]+)\\s*ms"),
       let match = regex.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)),
       let range = Range(match.range(at: 1), in: text) { rtt = Double(text[range]) }
    let success = process.terminationStatus == 0
    return ["network_rtt_ms": rtt, "network_loss": success ? 0 : 1, "network_connectivity": success ? 1 : 0]
}

let config = options()
let output: FileHandle
if let path = config.output {
    if !FileManager.default.fileExists(atPath: path) { FileManager.default.createFile(atPath: path, contents: nil) }
    guard let handle = FileHandle(forWritingAtPath: path) else { fail("Cannot open telemetry output") }
    handle.seekToEndOfFile()
    output = handle
} else { output = FileHandle.standardOutput }

// The process is bounded even without external supervision; TERM needs no persistent cleanup.
var lastTicks = cpuTicks()
var lastTime = ProcessInfo.processInfo.systemUptime
var lastCPU = consumedCPU()
let start = lastTime
var deadline = start
var sequence = 0
while ProcessInfo.processInfo.systemUptime - start < config.duration && (config.samples == nil || sequence < config.samples!) {
    let current = ProcessInfo.processInfo.systemUptime
    if current < deadline { usleep(UInt32(min(0.1, deadline - current) * 1e6)); continue }
    let timestamp = Date().timeIntervalSince1970
    var metrics: [String: Any] = [:]
    var quality: [String: Double] = [:]
    var provenance: [String: String] = [:]
    func add(_ key: String, _ value: Double?, _ source: String) {
        metrics[key] = nullOrNumber(value)
        quality[key] = value != nil && value!.isFinite ? 1 : 0
        provenance[key] = source
    }
    let ticks = cpuTicks()
    add("cpu_utilization", cpuFraction(lastTicks, ticks), "mach.host_statistics")
    add("cpu_count", Double(ProcessInfo.processInfo.processorCount), "Foundation.ProcessInfo.processorCount")
    var loads = [Double](repeating: 0, count: 3)
    let loadCount = getloadavg(&loads, 3)
    for (index, name) in ["cpu_load_1m", "cpu_load_5m", "cpu_load_15m"].enumerated() { add(name, loadCount > index ? loads[index] : nil, "posix.getloadavg") }
    for (key, value) in memoryMetrics() { add(key, value, key.hasPrefix("swap_") ? "sysctl.vm.swapusage" : "mach.host_statistics64.memory_proxy") }
    let state = ProcessInfo.processInfo.thermalState
    let thermal: Double?
    let thermalName: String
    switch state {
    case .nominal: thermal = 0; thermalName = "nominal"
    case .fair: thermal = 1.0 / 3; thermalName = "fair"
    case .serious: thermal = 2.0 / 3; thermalName = "serious"
    case .critical: thermal = 1; thermalName = "critical"
    @unknown default: thermal = nil; thermalName = "unknown"
    }
    add("thermal_pressure", thermal, "Foundation.ProcessInfo.thermalState")
    metrics["thermal_state"] = thermalName
    quality["thermal_state"] = thermal == nil ? 0 : 1
    provenance["thermal_state"] = "Foundation.ProcessInfo.thermalState"
    add("cpu_temperature_c", nil, "unavailable.public_api")
    for (key, value) in powerMetrics() { add(key, value, "IOKit.ps") }
    for (key, value) in peerMetrics(config.peerHost) { add(key, value, "icmp.one_echo") }
    add("uptime_s", current, "Foundation.ProcessInfo.systemUptime")
    let consumed = consumedCPU()
    add("daemon_cpu_fraction", sequence > 0 && current > lastTime ? max(0, (consumed - lastCPU) / (current - lastTime)) : nil, "posix.getrusage.self_and_children")
    add("daemon_rss_bytes", residentBytes(), "mach.task_info.resident_size")
    let record: [String: Any] = ["schema_version": "k0f.raw.v1", "source_kind": "real", "node_id": "mac", "sequence": sequence, "timestamp": timestamp, "monotonic_s": current, "sampling_interval_s": config.interval, "metrics": metrics, "quality": quality, "provenance": provenance]
    do {
        var data = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys, .withoutEscapingSlashes])
        data.append(10)
        try output.write(contentsOf: data)
    } catch { fail("Telemetry serialization or write failed") }
    lastTicks = ticks; lastTime = current; lastCPU = consumed
    sequence += 1
    deadline += config.interval
    if deadline < ProcessInfo.processInfo.systemUptime { deadline = ProcessInfo.processInfo.systemUptime }
}
if config.output != nil { try? output.close() }
