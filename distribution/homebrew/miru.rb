class Miru < Formula
  desc "AI-era secure PC fabric: remote desktop + AI agent + verifiable E2E + constellation"
  homepage "https://miru.app"
  license "MIT"
  version "0.1.0"

  on_macos do
    on_arm do
      url "https://github.com/shizukutanaka/miru/releases/download/v#{version}/miru-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
    on_intel do
      url "https://github.com/shizukutanaka/miru/releases/download/v#{version}/miru-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/shizukutanaka/miru/releases/download/v#{version}/miru-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
    on_intel do
      url "https://github.com/shizukutanaka/miru/releases/download/v#{version}/miru-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
  end

  def install
    bin.install "miru-host"
    bin.install "miru-mcp"
    bin.install "miru-signal"

    # Service definitions
    (etc/"miru").mkpath
    pkgshare.install "README.md", "LICENSE", "CHANGELOG.md"
  end

  service do
    run [opt_bin/"miru-host", "--headless"]
    keep_alive true
    log_path var/"log/miru-host.log"
    error_log_path var/"log/miru-host.err.log"
    environment_variables MIRU_LOG: "info"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/miru-host --version")
    assert_match "miru-mcp", shell_output("#{bin}/miru-mcp --help")
  end
end
