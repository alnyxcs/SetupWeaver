// runtime-admin/build.rs
#[cfg(windows)]
fn main() {
    const ADMIN_MANIFEST: &str = r#"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>
<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\">
  <assemblyIdentity version=\"1.0.0.0\" processorArchitecture=\"*\" name=\"SetupWeaver.Runtime.Admin\" type=\"win32\"/>
  <trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level=\"requireAdministrator\" uiAccess=\"false\"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

    let mut resource = winresource::WindowsResource::new();
    resource.set_manifest(ADMIN_MANIFEST);
    resource.compile().expect("failed to embed admin manifest");
}

#[cfg(not(windows))]
fn main() {}
