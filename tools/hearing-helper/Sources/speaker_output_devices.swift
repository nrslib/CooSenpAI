import CoreAudio
import Foundation

struct SpeakerOutputDevice: Encodable, Equatable {
    let id: String
    let name: String
    let objectID: AudioObjectID
    let outputStreams: [UInt]

    enum CodingKeys: String, CodingKey { case id, name }
}

enum SpeakerOutputDevices {
    static let aggregateUIDPrefix = "dev.nrslib.coosenpai.hearing."

    static func available() throws -> [SpeakerOutputDevice] {
        return try collect(objectIDs()) { id in
            guard try number(id, kAudioDevicePropertyDeviceIsAlive) == 1 else { return nil }
            let uid = try string(id, kAudioDevicePropertyDeviceUID)
            guard !uid.hasPrefix(aggregateUIDPrefix) else { return nil }
            let outputStreams = try outputStreamIndices(id)
            guard !outputStreams.isEmpty else { return nil }
            let name = try string(id, kAudioObjectPropertyName)
            return SpeakerOutputDevice(id: uid, name: name, objectID: id, outputStreams: outputStreams)
        }
    }

    static func objectIDs() throws -> [AudioObjectID] {
        let system = AudioObjectID(kAudioObjectSystemObject)
        var address = property(kAudioHardwarePropertyDevices)
        var size: UInt32 = 0
        try check(AudioObjectGetPropertyDataSize(system, &address, 0, nil, &size))
        let count = Int(size) / MemoryLayout<AudioObjectID>.size
        var ids = [AudioObjectID](repeating: 0, count: count)
        try check(AudioObjectGetPropertyData(system, &address, 0, nil, &size, &ids))
        return ids
    }

    static func collect(_ ids: [AudioObjectID], inspect: (AudioObjectID) throws -> SpeakerOutputDevice?) throws -> [SpeakerOutputDevice] {
        var devices: [SpeakerOutputDevice] = []
        for id in ids {
            if let device = try inspect(id) { devices.append(device) }
        }
        return devices.sorted { ($0.name, $0.id) < ($1.name, $1.id) }
    }

    static func resolve(_ selected: [String], from available: [SpeakerOutputDevice]) -> (devices: [SpeakerOutputDevice], missingCount: Int) {
        let byUID = Dictionary(available.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let devices = selected.compactMap { byUID[$0] }
        return (devices, selected.count - devices.count)
    }

    static func captureIdentity(_ devices: [SpeakerOutputDevice]) -> [String: AudioObjectID] {
        Dictionary(devices.filter { !$0.id.hasPrefix(aggregateUIDPrefix) }.map { ($0.id, $0.objectID) },
                   uniquingKeysWith: { first, _ in first })
    }

    static func captureStreams(_ devices: [SpeakerOutputDevice]) -> [(uid: String, stream: UInt)] {
        devices.flatMap { device in device.outputStreams.map { (device.id, $0) } }
    }

    static func outputStreamIndices(forChannelCounts counts: [UInt32]) -> [UInt] {
        counts.enumerated().compactMap { index, channels in channels > 0 ? UInt(index) : nil }
    }

    private static func outputStreamIndices(_ id: AudioObjectID) throws -> [UInt] {
        var address = property(kAudioDevicePropertyStreamConfiguration, scope: kAudioDevicePropertyScopeOutput)
        var size: UInt32 = 0
        try check(AudioObjectGetPropertyDataSize(id, &address, 0, nil, &size))
        guard size >= MemoryLayout<AudioBufferList>.size else { return [] }
        let storage = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { storage.deallocate() }
        let list = storage.bindMemory(to: AudioBufferList.self, capacity: 1)
        try check(AudioObjectGetPropertyData(id, &address, 0, nil, &size, list))
        let channelCounts = UnsafeMutableAudioBufferListPointer(list).map(\.mNumberChannels)
        return outputStreamIndices(forChannelCounts: channelCounts)
    }

    private static func number(_ id: AudioObjectID, _ selector: AudioObjectPropertySelector) throws -> UInt32 {
        var address = property(selector)
        var value: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        try check(AudioObjectGetPropertyData(id, &address, 0, nil, &size, &value))
        return value
    }

    private static func string(_ id: AudioObjectID, _ selector: AudioObjectPropertySelector) throws -> String {
        var address = property(selector)
        var value: CFString = "" as CFString
        var size = UInt32(MemoryLayout<CFString>.stride)
        try check(withUnsafeMutablePointer(to: &value) {
            AudioObjectGetPropertyData(id, &address, 0, nil, &size, $0)
        })
        return value as String
    }

    private static func property(_ selector: AudioObjectPropertySelector,
                                 scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    private static func check(_ status: OSStatus) throws {
        guard status == noErr else { throw SpeakerAudioTapError.operation("list-output-devices", status) }
    }
}

func runSpeakerOutputDevicesCommandIfRequested() -> Bool {
    guard CommandLine.arguments.count == 2, CommandLine.arguments[1] == "--list-output-devices" else { return false }
    do {
        let data = try JSONEncoder().encode(SpeakerOutputDevices.available())
        FileHandle.standardOutput.write(data)
    } catch {
        fputs("output-device-list failed\n", stderr)
        exit(1)
    }
    return true
}
