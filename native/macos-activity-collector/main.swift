import CryptoKit
import Dispatch
import EndpointSecurity
import Foundation

private func digest(_ value: String) -> String {
    let bytes = SHA256.hash(data: Data(value.utf8))
    return "sha256:" + bytes.map { String(format: "%02x", $0) }.joined()
}

private func tokenString(_ token: es_string_token_t) -> String {
    guard token.length > 0 else { return "" }
    return String(data: Data(bytes: token.data, count: Int(token.length)), encoding: .utf8) ?? ""
}

private func pathDigest(_ file: UnsafePointer<es_file_t>?) -> String? {
    guard let file else { return nil }
    return digest(tokenString(file.pointee.path))
}

private func emit(_ object: [String: Any]) {
    guard JSONSerialization.isValidJSONObject(object),
          let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else {
        return
    }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([0x0a]))
}

private func auditTokenMaterial(_ token: audit_token_t) -> String {
    withUnsafeBytes(of: token) { bytes in
        bytes.map { String(format: "%02x", $0) }.joined()
    }
}

private func base(_ message: UnsafePointer<es_message_t>, event: String) -> [String: Any] {
    let process = message.pointee.process.pointee
    var record: [String: Any] = [
        "event": event,
        "pid": audit_token_to_pid(process.audit_token),
        "parent_pid": process.ppid,
        "audit_token_digest": digest(auditTokenMaterial(process.audit_token)),
        "content_captured": false,
        "source_adapter": "endpoint-security",
    ]
    if let executable = pathDigest(process.executable) {
        record["executable_path_digest"] = executable
    }
    return record
}

private func handle(_ message: UnsafePointer<es_message_t>) {
    switch message.pointee.event_type {
    case ES_EVENT_TYPE_NOTIFY_EXEC:
        var record = base(message, event: "process.exec")
        let event = message.pointee.event.exec
        if let target = pathDigest(event.target.pointee.executable) {
            record["target_executable_path_digest"] = target
        }
        withUnsafePointer(to: event) { pointer in
            record["argument_count"] = es_exec_arg_count(pointer)
            record["environment_count"] = es_exec_env_count(pointer)
        }
        emit(record)
    case ES_EVENT_TYPE_NOTIFY_EXIT:
        var record = base(message, event: "process.exit")
        record["wait_status"] = message.pointee.event.exit.stat
        emit(record)
    case ES_EVENT_TYPE_NOTIFY_CLOSE:
        let event = message.pointee.event.close
        guard event.modified else { return }
        var record = base(message, event: "file.close_modified")
        if let target = pathDigest(event.target) {
            record["target_path_digest"] = target
        }
        record["modified"] = true
        emit(record)
    case ES_EVENT_TYPE_NOTIFY_UNLINK:
        var record = base(message, event: "file.unlink")
        if let target = pathDigest(message.pointee.event.unlink.target) {
            record["target_path_digest"] = target
        }
        emit(record)
    default:
        return
    }
}

private var client: OpaquePointer?
let result = es_new_client(&client) { _, message in
    handle(message)
}

guard result == ES_NEW_CLIENT_RESULT_SUCCESS, let client else {
    let reason: String
    switch result {
    case ES_NEW_CLIENT_RESULT_ERR_NOT_ENTITLED:
        reason = "endpoint_security_entitlement_missing"
    case ES_NEW_CLIENT_RESULT_ERR_NOT_PERMITTED:
        reason = "endpoint_security_user_approval_missing"
    case ES_NEW_CLIENT_RESULT_ERR_NOT_PRIVILEGED:
        reason = "endpoint_security_root_required"
    default:
        reason = "endpoint_security_client_initialization_failed"
    }
    emit(["event": "telemetry.unsupported", "event_class": "endpoint-security", "reason": reason])
    exit(78)
}

var events: [es_event_type_t] = [
    ES_EVENT_TYPE_NOTIFY_EXEC,
    ES_EVENT_TYPE_NOTIFY_EXIT,
    ES_EVENT_TYPE_NOTIFY_CLOSE,
    ES_EVENT_TYPE_NOTIFY_UNLINK,
]
let subscription = events.withUnsafeMutableBufferPointer { buffer in
    es_subscribe(client, buffer.baseAddress!, UInt32(buffer.count))
}
guard subscription == ES_RETURN_SUCCESS else {
    emit(["event": "telemetry.unsupported", "event_class": "endpoint-security", "reason": "subscription_failed"])
    es_delete_client(client)
    exit(78)
}

signal(SIGTERM, SIG_IGN)
let termination = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
termination.setEventHandler {
    es_unsubscribe_all(client)
    es_delete_client(client)
    exit(0)
}
termination.resume()
dispatchMain()
