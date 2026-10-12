IF DB_ID(N'sqlshop_test') IS NULL CREATE DATABASE sqlshop_test;
GO
IF DB_ID(N'sqlshop_other') IS NULL CREATE DATABASE sqlshop_other;
GO
USE sqlshop_test;
IF OBJECT_ID(N'dbo.navigation_probe', N'U') IS NULL
BEGIN
    CREATE TABLE dbo.navigation_probe (marker NVARCHAR(100) NOT NULL);
    INSERT INTO dbo.navigation_probe VALUES (N'sqlshop_test');
END;
GO
USE sqlshop_other;
IF OBJECT_ID(N'dbo.navigation_probe', N'U') IS NULL
BEGIN
    CREATE TABLE dbo.navigation_probe (marker NVARCHAR(100) NOT NULL);
    INSERT INTO dbo.navigation_probe VALUES (N'sqlshop_other');
END;
GO
