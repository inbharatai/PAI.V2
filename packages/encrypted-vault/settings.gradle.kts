pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
    plugins {
        kotlin("jvm") version "2.2.21"
        kotlin("plugin.serialization") version "2.2.21"
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        mavenCentral()
    }
}

rootProject.name = "unoone-pai-vault"

includeBuild("../core-contracts") {
    dependencySubstitution {
        substitute(module("com.unoone.pai:core-contracts")).using(project(":core-contracts"))
    }
}

include(":encrypted-vault")