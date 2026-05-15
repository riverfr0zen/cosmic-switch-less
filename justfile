# Name of the application's binary.
name := 'cosmic-switch-less'
# The unique ID of the application.
appid := 'com.github.riverfr0zen.cosmic-switch-less'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

# Application's appstream metadata
appdata := appid + '.metainfo.xml'
# Application's desktop entry
desktop := appid + '.desktop'
# Application's icon.
icon-svg := appid + '.svg'

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
appdata-dst := base-dir / 'share' / 'appdata' / appdata
bin-dst := base-dir / 'bin' / name
show-script-dst := base-dir / 'bin' / (name + '-show')
reload-script-dst := base-dir / 'bin' / (name + '-reload')
desktop-dst := base-dir / 'share' / 'applications' / desktop
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'
icon-svg-dst := icons-dst / 'scalable' / 'apps'

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build --locked {{ args }}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --all-features --locked {{ args }} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Run the application for testing purposes
run *args:
    env RUST_BACKTRACE=full cargo run --release --locked {{ args }}

# Installs locally only
install-local:
    # Stop the running daemon, if any, so cp can overwrite the binary
    # (Linux refuses with ETXTBSY otherwise). Next Alt+Tab cold-starts the
    # fresh binary via cosmic-switch-less-show.
    if pid=$(pidof cosmic-switch-less); then kill -TERM $pid; for _ in 1 2 3 4 5 6 7 8 9 10; do sleep 0.05; pidof cosmic-switch-less >/dev/null || break; done; fi
    cp target/release/cosmic-switch-less ~/.local/bin/
    cp scripts/cosmic-switch-less-show ~/.local/bin/
    cp scripts/cosmic-switch-less-reload ~/.local/bin/
    mkdir -p ~/.config/cosmic/{{ appid }}/v1
    test -e ~/.config/cosmic/{{ appid }}/v1/settings || cp resources/default-settings ~/.config/cosmic/{{ appid }}/v1/settings


# Installs files (system-wide; does not touch $HOME — packagers handle user
# config seeding via postinst hooks).
install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{ bin-dst }}
    install -Dm0755 {{ 'scripts' / (name + '-show') }} {{ show-script-dst }}
    install -Dm0755 {{ 'scripts' / (name + '-reload') }} {{ reload-script-dst }}
    install -Dm0644 {{ 'resources' / desktop }} {{ desktop-dst }}
    install -Dm0644 {{ 'resources' / appdata }} {{ appdata-dst }}
    install -Dm0644 {{ 'resources' / 'icons' / 'hicolor' / 'scalable' / 'apps' / 'icon.svg' }} {{ icon-svg-dst }}

# Uninstalls installed files (preserves the user's config directory).
uninstall:
    rm {{ bin-dst }} {{ show-script-dst }} {{ reload-script-dst }} {{ desktop-dst }} {{ icon-svg-dst }}

# Vendor dependencies locally
vendor:
    mkdir -p .cargo
    cargo vendor | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    tar pcf vendor.tar vendor
    rm -rf vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Bump cargo version, create git commit, and create tag
tag version:
    find -type f -name Cargo.toml -exec sed -i '0,/^version/s/^version.*/version = "{{ version }}"/' '{}' \; -exec git add '{}' \;
    cargo check
    cargo clean
    git add Cargo.lock
    git commit -m 'release: {{ version }}'
    git commit --amend
    git tag -a {{ version }} -m ''

# Bundle release tarball
[working-directory('target/release')]
bundle-release version arch:
    rm -rf {{ name }}-{{ version }}.{{ arch }}
    rm -rf {{ name }}-{{ version }}.{{ arch }}.tgz
    mkdir {{ name }}-{{ version }}.{{ arch }}
    cp cosmic-switch-less {{ name }}-{{ version }}.{{ arch }}/
    cp ../../scripts/* {{ name }}-{{ version }}.{{ arch }}/
    cp ../../resources/default-settings {{ name }}-{{ version }}.{{ arch }}/settings
    tar czvf {{ name }}-{{ version }}.{{ arch }}.tgz {{ name }}-{{ version }}.{{ arch }}
