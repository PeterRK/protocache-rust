#include <cstdlib>
#include <cstring>
#include <fstream>
#include <memory>
#include <set>
#include <string>
#include <vector>

#include <google/protobuf/compiler/parser.h>
#include <google/protobuf/compiler/importer.h>
#include <google/protobuf/descriptor.h>
#include <google/protobuf/descriptor.pb.h>
#include <google/protobuf/io/tokenizer.h>
#include <google/protobuf/io/zero_copy_stream_impl.h>

namespace {

class ErrorCollector final : public google::protobuf::compiler::MultiFileErrorCollector {
 public:
  void AddError(const std::string& filename, int line, int column,
                const std::string& message) override {
    if (!message_.empty()) {
      message_.append("\n");
    }
    message_.append(filename);
    message_.append(":");
    message_.append(std::to_string(line + 1));
    message_.append(":");
    message_.append(std::to_string(column + 1));
    message_.append(": ");
    message_.append(message);
  }

  const std::string& message() const { return message_; }

 private:
  std::string message_;
};

class ParseErrorCollector final : public google::protobuf::io::ErrorCollector {
 public:
  void AddError(int line, int column, const std::string& message) override {
    append("error", line, column, message);
  }

  void AddWarning(int line, int column, const std::string& message) override {
    append("warning", line, column, message);
  }

  const std::string& message() const { return message_; }

 private:
  void append(const char* kind, int line, int column, const std::string& message) {
    if (!message_.empty()) {
      message_.append("\n");
    }
    message_.append("[");
    message_.append(kind);
    message_.append("] line ");
    message_.append(std::to_string(line));
    message_.append(", column ");
    message_.append(std::to_string(column));
    message_.append(": ");
    message_.append(message);
  }

  std::string message_;
};

void collect_file(
    const google::protobuf::FileDescriptor* file,
    google::protobuf::FileDescriptorSet* out,
    std::set<std::string>* seen) {
  if (file == nullptr || !seen->insert(file->name()).second) {
    return;
  }

  for (int i = 0; i < file->dependency_count(); ++i) {
    collect_file(file->dependency(i), out, seen);
  }

  auto* proto = out->add_file();
  file->CopyTo(proto);
}

char* copy_string(const std::string& value) {
  char* result = static_cast<char*>(std::malloc(value.size() + 1));
  if (result == nullptr) {
    return nullptr;
  }
  std::memcpy(result, value.c_str(), value.size() + 1);
  return result;
}

uint8_t* copy_bytes(const std::string& value) {
  auto* result = static_cast<uint8_t*>(std::malloc(value.size()));
  if (result == nullptr) {
    return nullptr;
  }
  if (!value.empty()) {
    std::memcpy(result, value.data(), value.size());
  }
  return result;
}

template <typename StreamFactory>
int parse_proto_with_parser(
    StreamFactory&& stream_factory,
    const char* file_name,
    uint8_t** out_bytes,
    size_t* out_len,
    char** out_error) {
  google::protobuf::FileDescriptorProto descriptor;
  if (file_name != nullptr) {
    descriptor.set_name(file_name);
  }

  auto stream = stream_factory();
  if (!stream) {
    *out_error = copy_string("failed to open proto input");
    return 1;
  }

  ParseErrorCollector collector;
  google::protobuf::io::Tokenizer tokenizer(stream.get(), &collector);
  google::protobuf::compiler::Parser parser;
  if (!parser.Parse(&tokenizer, &descriptor)) {
    std::string message = collector.message();
    if (message.empty()) {
      message = "libprotoc parser failed";
    }
    *out_error = copy_string(message);
    return 1;
  }

  std::string serialized;
  if (!descriptor.SerializeToString(&serialized)) {
    *out_error = copy_string("libprotoc failed to serialize descriptor");
    return 1;
  }

  uint8_t* bytes = copy_bytes(serialized);
  if (serialized.size() > 0 && bytes == nullptr) {
    *out_error = copy_string("out of memory copying descriptor bytes");
    return 1;
  }
  *out_bytes = bytes;
  *out_len = serialized.size();
  return 0;
}

}  // namespace

extern "C" int protocache_parse_proto(
    const char* source,
    const char* file_name,
    uint8_t** out_bytes,
    size_t* out_len,
    char** out_error) {
  if (out_bytes == nullptr || out_len == nullptr || out_error == nullptr ||
      source == nullptr) {
    return 1;
  }

  *out_bytes = nullptr;
  *out_len = 0;
  *out_error = nullptr;

  return parse_proto_with_parser(
      [source]() {
        return std::make_unique<google::protobuf::io::ArrayInputStream>(
            source, static_cast<int>(std::strlen(source)));
      },
      file_name,
      out_bytes,
      out_len,
      out_error);
}

extern "C" int protocache_parse_proto_file(
    const char* path,
    uint8_t** out_bytes,
    size_t* out_len,
    char** out_error) {
  if (out_bytes == nullptr || out_len == nullptr || out_error == nullptr ||
      path == nullptr) {
    return 1;
  }

  *out_bytes = nullptr;
  *out_len = 0;
  *out_error = nullptr;

  google::protobuf::FileDescriptorProto descriptor;
  descriptor.set_name(path);

  std::ifstream input(path);
  if (!input) {
    *out_error = copy_string(std::string("fail to open: ") + path);
    return 1;
  }

  ParseErrorCollector collector;
  google::protobuf::io::IstreamInputStream stream(&input);
  google::protobuf::io::Tokenizer tokenizer(&stream, &collector);
  google::protobuf::compiler::Parser parser;
  if (!parser.Parse(&tokenizer, &descriptor)) {
    std::string message = collector.message();
    if (message.empty()) {
      message = "libprotoc parser failed";
    }
    *out_error = copy_string(message);
    return 1;
  }

  std::string serialized;
  if (!descriptor.SerializeToString(&serialized)) {
    *out_error = copy_string("libprotoc failed to serialize descriptor");
    return 1;
  }

  uint8_t* bytes = copy_bytes(serialized);
  if (serialized.size() > 0 && bytes == nullptr) {
    *out_error = copy_string("out of memory copying descriptor bytes");
    return 1;
  }
  *out_bytes = bytes;
  *out_len = serialized.size();
  return 0;
}

extern "C" int protocache_parse_proto_file_set(
    const char* proto_path,
    const char* const* extra_import_paths,
    size_t extra_import_count,
    uint8_t** out_bytes,
    size_t* out_len,
    char** out_error) {
  if (out_bytes == nullptr || out_len == nullptr || out_error == nullptr ||
      proto_path == nullptr) {
    return 1;
  }

  *out_bytes = nullptr;
  *out_len = 0;
  *out_error = nullptr;

  std::string proto_file(proto_path);
  std::string::size_type slash = proto_file.find_last_of("/\\");
  std::string proto_dir = slash == std::string::npos ? "." : proto_file.substr(0, slash);
  std::string virtual_file =
      slash == std::string::npos ? proto_file : proto_file.substr(slash + 1);

  google::protobuf::compiler::DiskSourceTree source_tree;
  source_tree.MapPath("", proto_dir);
  for (size_t i = 0; i < extra_import_count; ++i) {
    if (extra_import_paths[i] != nullptr) {
      source_tree.MapPath("", extra_import_paths[i]);
    }
  }

  ErrorCollector collector;
  google::protobuf::compiler::Importer importer(&source_tree, &collector);
  const google::protobuf::FileDescriptor* file = importer.Import(virtual_file);
  if (file == nullptr) {
    std::string message = collector.message();
    if (message.empty()) {
      message = "libprotoc failed to import " + virtual_file;
    }
    *out_error = copy_string(message);
    return 1;
  }

  google::protobuf::FileDescriptorSet descriptor_set;
  std::set<std::string> seen;
  collect_file(file, &descriptor_set, &seen);

  std::string serialized;
  if (!descriptor_set.SerializeToString(&serialized)) {
    *out_error = copy_string("libprotoc failed to serialize descriptor set");
    return 1;
  }

  uint8_t* bytes = copy_bytes(serialized);
  if (serialized.size() > 0 && bytes == nullptr) {
    *out_error = copy_string("out of memory copying descriptor set bytes");
    return 1;
  }

  *out_bytes = bytes;
  *out_len = serialized.size();
  return 0;
}

extern "C" void protocache_free_proto_bytes(uint8_t* bytes) {
  std::free(bytes);
}

extern "C" void protocache_free_proto_error(char* error) {
  std::free(error);
}
