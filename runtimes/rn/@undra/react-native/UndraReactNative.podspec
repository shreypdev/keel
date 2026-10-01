# The iOS half of @undra/react-native (ADR-038): the C++ TurboModule over the Undra C ABI.
#
# The core itself comes from the `UndraCore` pod that `undra build --platform rn` writes next to
# `UndraCore.xcframework` (build/ios/UndraCore.podspec); the app's Podfile points at it:
#
#   pod 'UndraCore', :path => '../../build/ios'
require "json"

package = JSON.parse(File.read(File.join(__dir__, "package.json")))

Pod::Spec.new do |s|
  s.name         = "UndraReactNative"
  s.version      = package["version"]
  s.summary      = package["description"]
  s.homepage     = "https://github.com/shreypdev/undra"
  s.license      = package["license"]
  s.authors      = "The Undra authors"
  s.platforms    = { :ios => min_ios_version_supported }
  s.source       = { :git => "https://github.com/shreypdev/undra.git", :tag => "v#{s.version}" }

  s.source_files = "cpp/*.{h,cpp}", "ios/*.{h,mm}"
  s.frameworks   = "QuartzCore"
  s.pod_target_xcconfig = {
    "CLANG_CXX_LANGUAGE_STANDARD" => "c++20",
  }

  s.dependency "UndraCore"
  install_modules_dependencies(s)
end
