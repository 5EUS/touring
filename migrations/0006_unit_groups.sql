-- Add unit upload groups to chapters and episodes
ALTER TABLE chapters ADD COLUMN "upload_group" TEXT;
ALTER TABLE episodes ADD COLUMN "upload_group" TEXT;
