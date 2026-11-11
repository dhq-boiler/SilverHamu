Gem::Specification.new do |s|
  s.name = "silverhamu-client"
  s.version = "0.1.0"
  s.summary = "Ruby client for the silverhamu TCP service"
  s.description = "Read and write binary keys and values, commit transactions, and connect to silverhamu with token or TLS account authentication."
  s.authors = ["silverhamu contributors"]
  s.homepage = "https://github.com/dhq-boiler/catdb"
  s.license = "MIT"
  s.metadata = {
    "source_code_uri" => "https://github.com/dhq-boiler/catdb/tree/clients/ruby/v#{s.version}/clients/ruby",
    "bug_tracker_uri" => "https://github.com/dhq-boiler/catdb/issues",
    "allowed_push_host" => "https://rubygems.org"
  }
  s.files = ["lib/silverhamu.rb", "README.md", "LICENSE"]
  s.require_paths = ["lib"]
  s.required_ruby_version = ">= 3.1"
end
