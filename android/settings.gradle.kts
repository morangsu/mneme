// mneme 安卓工程：单 module（:app），领域逻辑全部在 Rust 侧，本工程只做 UI/调度。
pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()      // AndroidX / ML Kit
        mavenCentral() // Kotlin / coroutines
    }
}

rootProject.name = "mneme"
include(":app")
